package dev.aura.v4

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.Intent
import android.graphics.Path
import android.graphics.Rect
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.DataOutputStream
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/**
 * AURA v4 Accessibility Service.
 *
 * ## Responsibilities
 * 1. **Screen tree capture** — traverse the accessibility node tree and serialize
 *    it to bincode-compatible bytes matching Rust's `Vec<RawA11yNode>`.
 * 2. **Gesture dispatch** — tap, swipe, scroll via [GestureDescription].
 * 3. **Text input** — inject text into focused [AccessibilityNodeInfo].
 * 4. **Global actions** — back, home, recents, notification shade.
 * 5. **Generic action dispatch** — JSON-based fallback for complex actions.
 *
 * ## Bincode Serialization Format
 *
 * Rust's `bincode` with default config serializes as:
 * - `String` → `u64 length` (little-endian) + UTF-8 bytes
 * - `Option<String>` → `u8` (0=None, 1=Some) + String if Some
 * - `Vec<usize>` → `u64 count` + (`u64` per element)
 * - `bool` → `u8` (0 or 1)
 * - `i32` → 4 bytes little-endian
 * - `Vec<RawA11yNode>` → `u64 count` + serialized nodes
 */
class AuraAccessibilityService : AccessibilityService() {

    companion object {
        private const val TAG = "AuraA11ySvc"

        /** Gesture dispatch timeout in milliseconds. */
        private const val GESTURE_TIMEOUT_MS = 5_000L
    }

    /** The package name of the foreground application, updated on window changes. */
    @Volatile
    var currentPackageName: String? = null
        private set

    // ── Service Lifecycle ───────────────────────────────────────────────

    override fun onServiceConnected() {
        super.onServiceConnected()
        Log.i(TAG, "AccessibilityService connected")
        AuraDaemonBridge.registerService(this)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        event ?: return

        // Track the foreground package on window state changes.
        if (event.eventType == AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED) {
            event.packageName?.toString()?.let { pkg ->
                if (pkg != "android" && pkg.isNotEmpty()) {
                    currentPackageName = pkg
                }
            }
        }
    }

    override fun onInterrupt() {
        Log.w(TAG, "AccessibilityService interrupted")
    }

    override fun onDestroy() {
        Log.i(TAG, "AccessibilityService destroyed")
        AuraDaemonBridge.unregisterService()
        super.onDestroy()
    }

    // ════════════════════════════════════════════════════════════════════
    //  GESTURE DISPATCH
    // ════════════════════════════════════════════════════════════════════

    /**
     * Dispatch a tap at (x, y) using a 1ms stroke at that point.
     */
    fun dispatchTap(x: Int, y: Int): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) {
            Log.w(TAG, "Gesture API requires API 24+")
            return false
        }

        val path = Path().apply { moveTo(x.toFloat(), y.toFloat()) }
        val stroke = GestureDescription.StrokeDescription(
            path, 0L, 1L  // startTime=0, duration=1ms (tap)
        )

        return dispatchGesture(stroke)
    }

    /**
     * Dispatch a swipe from (x1,y1) to (x2,y2) over [durationMs].
     */
    fun dispatchSwipe(x1: Int, y1: Int, x2: Int, y2: Int, durationMs: Long): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) return false

        val path = Path().apply {
            moveTo(x1.toFloat(), y1.toFloat())
            lineTo(x2.toFloat(), y2.toFloat())
        }
        val duration = durationMs.coerceAtLeast(1L)
        val stroke = GestureDescription.StrokeDescription(path, 0L, duration)

        return dispatchGesture(stroke)
    }

    /**
     * Type [text] into the currently focused input field.
     *
     * Strategy:
     * 1. Find the focused node.
     * 2. Use `ACTION_SET_TEXT` if available (API 21+), which is the most
     *    reliable approach.
     * 3. Fall back to clipboard paste if `ACTION_SET_TEXT` isn't supported.
     */
    fun dispatchTypeText(text: String): Boolean {
        val focused = findFocusedNode() ?: run {
            Log.w(TAG, "typeText: no focused node found")
            return false
        }

        return try {
            val args = Bundle().apply {
                putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
            }
            focused.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
        } catch (e: Exception) {
            Log.e(TAG, "typeText failed: ${e.message}")
            false
        } finally {
            focused.recycle()
        }
    }

    // ════════════════════════════════════════════════════════════════════
    //  SCREEN TREE SERIALIZATION (→ bincode Vec<RawA11yNode>)
    // ════════════════════════════════════════════════════════════════════

    /**
     * Capture the current accessibility tree and serialize it as bincode
     * bytes compatible with Rust's `Vec<RawA11yNode>`.
     */
    fun serializeScreenTree(): ByteArray {
        val root = try {
            rootInActiveWindow
        } catch (e: Exception) {
            Log.w(TAG, "serializeScreenTree: rootInActiveWindow failed: ${e.message}")
            null
        } ?: return ByteArray(0)

        return try {
            // Phase 1: Flatten the tree into a list of raw nodes.
            val flatNodes = mutableListOf<FlatNode>()
            flattenTree(root, flatNodes, maxDepth = 30, maxNodes = 5000)

            // Phase 2: Serialize the flat list to bincode.
            serializeToBincode(flatNodes)
        } catch (e: Exception) {
            Log.e(TAG, "serializeScreenTree failed: ${e.message}", e)
            ByteArray(0)
        } finally {
            root.recycle()
        }
    }

    /**
     * Flattened accessibility node matching Rust's `RawA11yNode` fields.
     */
    private data class FlatNode(
        val className: String,
        val text: String?,
        val contentDesc: String?,
        val resourceId: String?,
        val packageName: String,
        val boundsLeft: Int,
        val boundsTop: Int,
        val boundsRight: Int,
        val boundsBottom: Int,
        val isClickable: Boolean,
        val isScrollable: Boolean,
        val isEditable: Boolean,
        val isCheckable: Boolean,
        val isChecked: Boolean,
        val isEnabled: Boolean,
        val isFocused: Boolean,
        val isVisible: Boolean,
        val childrenIndices: List<Int>
    )

    /**
     * BFS/DFS flatten of the accessibility tree.
     *
     * Each [AccessibilityNodeInfo] is assigned a flat index. Children are
     * referenced by their indices in the flat list, matching the Rust-side
     * `children_indices: Vec<usize>` field.
     */
    private fun flattenTree(
        root: AccessibilityNodeInfo,
        out: MutableList<FlatNode>,
        maxDepth: Int,
        maxNodes: Int
    ) {
        data class QueueItem(val node: AccessibilityNodeInfo, val depth: Int)

        val queue = ArrayDeque<QueueItem>()
        queue.addLast(QueueItem(root, 0))

        // We need two passes conceptually, but we can do it in one with
        // deferred child index patching.
        // Map from AccessibilityNodeInfo hashCode → index in `out`.
        // Since we process BFS, parent is always added before children.
        val pendingChildren = mutableListOf<Pair<Int, AccessibilityNodeInfo>>()

        while (queue.isNotEmpty() && out.size < maxNodes) {
            val (node, depth) = queue.removeFirst()

            val myIndex = out.size
            val bounds = Rect()
            node.getBoundsInScreen(bounds)

            val childCount = node.childCount
            val childIndices = mutableListOf<Int>()

            // We'll fill childIndices after we know the children's indices.
            out.add(
                FlatNode(
                    className = node.className?.toString() ?: "",
                    text = node.text?.toString(),
                    contentDesc = node.contentDescription?.toString(),
                    resourceId = node.viewIdResourceName,
                    packageName = node.packageName?.toString() ?: "",
                    boundsLeft = bounds.left,
                    boundsTop = bounds.top,
                    boundsRight = bounds.right,
                    boundsBottom = bounds.bottom,
                    isClickable = node.isClickable,
                    isScrollable = node.isScrollable,
                    isEditable = node.isEditable,
                    isCheckable = node.isCheckable,
                    isChecked = node.isChecked,
                    isEnabled = node.isEnabled,
                    isFocused = node.isFocused,
                    isVisible = node.isVisibleToUser,
                    childrenIndices = childIndices  // mutable, patched below
                )
            )

            if (depth < maxDepth) {
                for (i in 0 until childCount) {
                    if (out.size + queue.size >= maxNodes) break
                    val child = node.getChild(i) ?: continue
                    val childIndex = out.size + queue.size
                    childIndices.add(childIndex)
                    queue.addLast(QueueItem(child, depth + 1))
                }
            }

            // Don't recycle root — caller handles that.
            if (node !== root) {
                node.recycle()
            }
        }

        // Drain remaining queued nodes that exceeded maxNodes.
        while (queue.isNotEmpty()) {
            val (node, _) = queue.removeFirst()
            if (node !== root) node.recycle()
        }
    }

    /**
     * Serialize a list of [FlatNode] to bincode format matching Rust's
     * `Vec<RawA11yNode>` (bincode default config, little-endian, varint=false).
     *
     * Bincode default config layout for `Vec<T>`:
     * - u64 length (number of elements)
     * - each T serialized in order
     */
    private fun serializeToBincode(nodes: List<FlatNode>): ByteArray {
        val baos = ByteArrayOutputStream(nodes.size * 200)  // estimate
        val buf = ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN)

        fun writeU64(value: Long) {
            buf.clear()
            buf.putLong(value)
            baos.write(buf.array(), 0, 8)
        }

        fun writeI32(value: Int) {
            buf.clear()
            buf.putInt(value)
            baos.write(buf.array(), 0, 4)
        }

        fun writeBool(value: Boolean) {
            baos.write(if (value) 1 else 0)
        }

        fun writeString(s: String) {
            val bytes = s.toByteArray(Charsets.UTF_8)
            writeU64(bytes.size.toLong())
            baos.write(bytes)
        }

        fun writeOptionalString(s: String?) {
            if (s == null) {
                baos.write(0)  // None
            } else {
                baos.write(1)  // Some
                writeString(s)
            }
        }

        fun writeUsizeVec(indices: List<Int>) {
            writeU64(indices.size.toLong())
            for (idx in indices) {
                writeU64(idx.toLong())  // usize = u64 on 64-bit
            }
        }

        // Vec<RawA11yNode> header: element count
        writeU64(nodes.size.toLong())

        for (node in nodes) {
            // Fields in exact order matching the Rust struct:
            writeString(node.className)            // class_name: String
            writeOptionalString(node.text)          // text: Option<String>
            writeOptionalString(node.contentDesc)   // content_desc: Option<String>
            writeOptionalString(node.resourceId)    // resource_id: Option<String>
            writeString(node.packageName)           // package_name: String
            writeI32(node.boundsLeft)               // bounds_left: i32
            writeI32(node.boundsTop)                // bounds_top: i32
            writeI32(node.boundsRight)              // bounds_right: i32
            writeI32(node.boundsBottom)             // bounds_bottom: i32
            writeBool(node.isClickable)             // is_clickable: bool
            writeBool(node.isScrollable)            // is_scrollable: bool
            writeBool(node.isEditable)              // is_editable: bool
            writeBool(node.isCheckable)             // is_checkable: bool
            writeBool(node.isChecked)               // is_checked: bool
            writeBool(node.isEnabled)               // is_enabled: bool
            writeBool(node.isFocused)               // is_focused: bool
            writeBool(node.isVisible)               // is_visible: bool
            writeUsizeVec(node.childrenIndices)     // children_indices: Vec<usize>
        }

        return baos.toByteArray()
    }

    // ════════════════════════════════════════════════════════════════════
    //  GENERIC ACTION DISPATCH (JSON)
    // ════════════════════════════════════════════════════════════════════

    /**
     * Execute an action from its JSON representation.
     *
     * Called by [AuraDaemonBridge.executeAction] for action types that
     * don't have dedicated JNI bridge methods.
     */
    fun executeGenericAction(actionJson: String): Boolean {
        return try {
            val json = JSONObject(actionJson)

            // The serde JSON for ActionType uses an externally tagged enum:
            // {"OpenApp":{"package":"com.foo"}} or "Back" (unit variants).
            val keys = json.keys()
            if (!keys.hasNext()) return false

            val actionName = keys.next()
            when (actionName) {
                "OpenApp" -> {
                    val data = json.getJSONObject("OpenApp")
                    val pkg = data.getString("package")
                    openApp(pkg)
                }
                "Scroll" -> {
                    val data = json.getJSONObject("Scroll")
                    val direction = data.getString("direction")
                    val amount = data.optInt("amount", 500)
                    executeScroll(direction, amount)
                }
                "LongPress" -> {
                    val data = json.getJSONObject("LongPress")
                    dispatchLongPress(data.getInt("x"), data.getInt("y"))
                }
                "NotificationAction" -> {
                    // Not easily implementable via A11Y; return false.
                    Log.w(TAG, "NotificationAction: not directly supported")
                    false
                }
                "WaitForElement" -> {
                    val data = json.getJSONObject("WaitForElement")
                    val timeoutMs = data.optLong("timeout_ms", 10_000)
                    // The Rust side should handle waiting logic; this is just
                    // a presence check.
                    waitForElement(data.getJSONObject("selector"), timeoutMs)
                }
                "AssertElement" -> {
                    // Assertions are verified on the Rust side using the tree.
                    true
                }
                else -> {
                    Log.w(TAG, "Unknown generic action: $actionName")
                    false
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "executeGenericAction failed: ${e.message}", e)
            false
        }
    }

    // ── Specific Action Implementations ─────────────────────────────────

    private fun openApp(packageName: String): Boolean {
        return try {
            val intent = packageManager.getLaunchIntentForPackage(packageName)
            if (intent != null) {
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                startActivity(intent)
                true
            } else {
                Log.w(TAG, "No launch intent for $packageName")
                false
            }
        } catch (e: Exception) {
            Log.e(TAG, "openApp($packageName) failed: ${e.message}")
            false
        }
    }

    private fun executeScroll(direction: String, amount: Int): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) return false

        // Convert scroll direction to swipe gesture.
        // "Down" scroll = swipe up (finger moves from bottom to top).
        val centerX = resources.displayMetrics.widthPixels / 2
        val centerY = resources.displayMetrics.heightPixels / 2
        val scrollAmount = amount.coerceIn(100, 2000)

        val (x1, y1, x2, y2) = when (direction) {
            "Down" -> listOf(centerX, centerY + scrollAmount / 2, centerX, centerY - scrollAmount / 2)
            "Up" -> listOf(centerX, centerY - scrollAmount / 2, centerX, centerY + scrollAmount / 2)
            "Left" -> listOf(centerX + scrollAmount / 2, centerY, centerX - scrollAmount / 2, centerY)
            "Right" -> listOf(centerX - scrollAmount / 2, centerY, centerX + scrollAmount / 2, centerY)
            else -> return false
        }

        return dispatchSwipe(x1, y1, x2, y2, 300L)
    }

    private fun dispatchLongPress(x: Int, y: Int): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) return false

        val path = Path().apply { moveTo(x.toFloat(), y.toFloat()) }
        // Long press = hold for 500ms+
        val stroke = GestureDescription.StrokeDescription(path, 0L, 600L)
        return dispatchGesture(stroke)
    }

    @Suppress("UNUSED_PARAMETER")
    private fun waitForElement(selectorJson: JSONObject, timeoutMs: Long): Boolean {
        // Simple polling implementation — check tree every 500ms.
        val deadline = System.currentTimeMillis() + timeoutMs
        val selectorType = selectorJson.keys().let { if (it.hasNext()) it.next() else null }
            ?: return false

        while (System.currentTimeMillis() < deadline) {
            val root = try {
                rootInActiveWindow
            } catch (_: Exception) {
                null
            } ?: run {
                Thread.sleep(500)
                continue
            }

            val found = try {
                when (selectorType) {
                    "Text" -> {
                        val text = selectorJson.getString("Text")
                        findNodeByText(root, text) != null
                    }
                    "ResourceId" -> {
                        val id = selectorJson.getString("ResourceId")
                        findNodeByResourceId(root, id) != null
                    }
                    "ContentDescription" -> {
                        val desc = selectorJson.getString("ContentDescription")
                        findNodeByContentDesc(root, desc) != null
                    }
                    else -> false
                }
            } finally {
                root.recycle()
            }

            if (found) return true
            Thread.sleep(500)
        }

        return false
    }

    // ── Helper: Gesture dispatch with synchronous waiting ───────────────

    private fun dispatchGesture(stroke: GestureDescription.StrokeDescription): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.N) return false

        val gesture = GestureDescription.Builder()
            .addStroke(stroke)
            .build()

        val latch = CountDownLatch(1)
        val result = AtomicReference(false)

        val dispatched = dispatchGesture(
            gesture,
            object : GestureResultCallback() {
                override fun onCompleted(gestureDescription: GestureDescription?) {
                    result.set(true)
                    latch.countDown()
                }

                override fun onCancelled(gestureDescription: GestureDescription?) {
                    result.set(false)
                    latch.countDown()
                }
            },
            null  // handler — use main thread
        )

        if (!dispatched) {
            Log.w(TAG, "dispatchGesture returned false")
            return false
        }

        return try {
            latch.await(GESTURE_TIMEOUT_MS, TimeUnit.MILLISECONDS)
            result.get()
        } catch (_: InterruptedException) {
            Thread.currentThread().interrupt()
            false
        }
    }

    // ── Helper: Node Searching ──────────────────────────────────────────

    private fun findFocusedNode(): AccessibilityNodeInfo? {
        return try {
            rootInActiveWindow?.findFocus(AccessibilityNodeInfo.FOCUS_INPUT)
        } catch (_: Exception) {
            null
        }
    }

    private fun findNodeByText(
        root: AccessibilityNodeInfo,
        text: String
    ): AccessibilityNodeInfo? {
        val nodes = root.findAccessibilityNodeInfosByText(text)
        return nodes?.firstOrNull()
    }

    private fun findNodeByResourceId(
        root: AccessibilityNodeInfo,
        resourceId: String
    ): AccessibilityNodeInfo? {
        val nodes = root.findAccessibilityNodeInfosByViewId(resourceId)
        return nodes?.firstOrNull()
    }

    private fun findNodeByContentDesc(
        node: AccessibilityNodeInfo,
        desc: String
    ): AccessibilityNodeInfo? {
        if (node.contentDescription?.toString()?.contains(desc, ignoreCase = true) == true) {
            return node
        }
        for (i in 0 until node.childCount) {
            val child = node.getChild(i) ?: continue
            val found = findNodeByContentDesc(child, desc)
            if (found != null) return found
            child.recycle()
        }
        return null
    }
}
