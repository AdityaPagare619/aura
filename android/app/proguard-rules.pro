# Keep JNI bridge methods called from native code via reflection
-keep class dev.aura.v4.AuraDaemonBridge {
    public static <methods>;
}

# Keep accessibility service
-keep class dev.aura.v4.AuraAccessibilityService { *; }
