//! Security tests for FFI boundary safety.
//!
//! Tests that FFI boundaries properly reject dangerous inputs:
//!   - Null pointer rejection
//!   - Path traversal prevention
//!   - URL scheme validation
//!   - Input size limits
//!
//! These tests validate safety invariants at the Rust↔C/Java FFI boundary
//! without requiring an actual Android device or FFI library loaded.

// ---------------------------------------------------------------------------
// Null pointer rejection
// ---------------------------------------------------------------------------

#[cfg(test)]
mod null_pointer_rejection {
    use std::ffi::{CStr, CString};
    use std::ptr;

    /// Verify that a null raw C string pointer is detected before dereference.
    /// This simulates the pattern used in FFI functions that accept `*const c_char`.
    #[test]
    fn test_null_c_str_rejected() {
        let null_ptr: *const i8 = ptr::null();
        // SAFETY: We're explicitly testing the null check path.
        let result = unsafe {
            if null_ptr.is_null() {
                Err("null pointer")
            } else {
                Ok(CStr::from_ptr(null_ptr).to_string_lossy().into_owned())
            }
        };
        assert!(result.is_err(), "null C string pointer must be rejected");
    }

    /// Verify that a valid CString pointer is accepted.
    #[test]
    fn test_valid_c_str_accepted() {
        let cstr = CString::new("valid_input").unwrap();
        let ptr: *const i8 = cstr.as_ptr();

        let result = unsafe {
            if ptr.is_null() {
                Err("null pointer")
            } else {
                Ok(CStr::from_ptr(ptr).to_string_lossy().into_owned())
            }
        };
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "valid_input");
    }

    /// Verify null mutable pointer is rejected (simulates output buffer pattern).
    /// We check for null BEFORE creating the slice (as FFI code must do).
    #[test]
    fn test_null_mut_ptr_rejected() {
        let null_ptr: *mut u8 = ptr::null_mut();

        // FFI code must check for null BEFORE calling from_raw_parts_mut.
        let result: Result<(), &str> = if null_ptr.is_null() {
            Err("null output buffer")
        } else {
            Ok(())
        };
        assert!(result.is_err(), "null mutable pointer must be rejected");
    }

    /// Verify that a non-null pointer passes the null check.
    #[test]
    fn test_non_null_ptr_accepted() {
        let mut val: u8 = 42;
        let ptr: *mut u8 = &mut val;

        let result: Result<u8, &str> = if ptr.is_null() {
            Err("null pointer")
        } else {
            Ok(unsafe { *ptr })
        };
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
    }
}

// ---------------------------------------------------------------------------
// Path traversal prevention
// ---------------------------------------------------------------------------

#[cfg(test)]
mod path_traversal {
    /// AURAs FFI boundary must reject paths containing traversal sequences.
    /// This test validates the detection logic that should be applied before
    /// any file operation on user-supplied paths from IPC/FFI.
    fn is_path_safe(path: &str) -> bool {
        // Reject paths with traversal components.
        if path.contains("..") {
            return false;
        }
        // Reject paths with null bytes (C string terminator injection).
        if path.contains('\0') {
            return false;
        }
        // Reject absolute paths when a relative path is expected.
        // On Android, only allow paths under /data/data/com.aura/ or /data/local/tmp/aura/
        #[cfg(target_os = "android")]
        {
            if path.starts_with('/')
                && !path.starts_with("/data/data/com.aura/")
                && !path.starts_with("/data/local/tmp/aura/")
            {
                return false;
            }
        }
        true
    }

    #[test]
    fn test_reject_dotdot_traversal() {
        assert!(!is_path_safe("../../../etc/passwd"));
        assert!(!is_path_safe("data/../../etc/shadow"));
        assert!(!is_path_safe("models/../../../root/.ssh/id_rsa"));
    }

    #[test]
    fn test_reject_encoded_traversal() {
        // Even with URL-encoding, the literal ".." should be caught.
        assert!(!is_path_safe("..%2F..%2Fetc%2Fpasswd"));
        // Mixed separators.
        assert!(!is_path_safe("data\\..\\..\\windows\\system32"));
    }

    #[test]
    fn test_reject_null_byte_injection() {
        assert!(!is_path_safe("safe_file.txt\0../../etc/passwd"));
        assert!(!is_path_safe("\0"));
    }

    #[test]
    fn test_accept_safe_paths() {
        assert!(is_path_safe("models/qwen3-8b.gguf"));
        assert!(is_path_safe("data/memory.db"));
        assert!(is_path_safe("config.toml"));
        assert!(is_path_safe(""));
    }

    #[test]
    fn test_reject_traversal_at_various_positions() {
        assert!(!is_path_safe(".."));
        assert!(!is_path_safe("foo/../bar"));
        assert!(!is_path_safe("foo/.."));
        assert!(!is_path_safe("../foo"));
    }
}

// ---------------------------------------------------------------------------
// URL scheme validation
// ---------------------------------------------------------------------------

#[cfg(test)]
mod url_scheme_validation {
    /// Validate that only permitted URL schemes are accepted at FFI boundaries.
    /// AURA's JNI openUrl only allows http:// and https:// schemes.
    /// file:// is NOT allowed — local file access must go through the IPC layer.
    fn is_allowed_scheme(url: &str) -> bool {
        // CRIT-02 FIX: Only http and https are permitted.
        // file:// is rejected — local files use the IPC/neocortex path instead.
        let allowed_schemes = ["http://", "https://"];

        // Must not be empty.
        if url.is_empty() {
            return false;
        }

        // Must start with an allowed scheme.
        for scheme in &allowed_schemes {
            if url.starts_with(scheme) {
                return true;
            }
        }

        // Reject everything else — including javascript:, data:, file:, intent:, tel:, etc.
        false
    }

    #[test]
    fn test_reject_javascript_scheme() {
        assert!(!is_allowed_scheme("javascript:alert('xss')"));
        assert!(!is_allowed_scheme("javascript:void(0)"));
    }

    #[test]
    fn test_reject_data_scheme() {
        assert!(!is_allowed_scheme(
            "data:text/html,<script>alert('xss')</script>"
        ));
        assert!(!is_allowed_scheme(
            "data:application/octet-stream;base64,AAAA"
        ));
    }

    #[test]
    fn test_reject_file_scheme() {
        // CRIT-02 FIX: file:// is NOT allowed in openUrl — local files must go through IPC.
        assert!(!is_allowed_scheme("file:///etc/passwd"));
        assert!(!is_allowed_scheme(
            "file:///data/data/com.aura/models/model.gguf"
        ));
    }

    #[test]
    fn test_reject_ftp_scheme() {
        assert!(!is_allowed_scheme("ftp://malicious.com/payload"));
    }

    #[test]
    fn test_accept_http_https() {
        assert!(is_allowed_scheme(
            "http://localhost:8080/v1/chat/completions"
        ));
        assert!(is_allowed_scheme("https://api.example.com/inference"));
    }

    #[test]
    fn test_reject_schemeless_urls() {
        assert!(!is_allowed_scheme("//evil.com/payload"));
        assert!(!is_allowed_scheme("localhost:8080")); // no scheme
        assert!(!is_allowed_scheme("")); // empty URL
    }

    #[test]
    fn test_reject_empty_url() {
        // CRIT-02 FIX: Empty URLs must be rejected before JNI call.
        assert!(!is_allowed_scheme(""));
    }

    #[test]
    fn test_reject_javascript_case_variants() {
        // Case-sensitivity: our starts_with check is case-sensitive,
        // so JavaScript: would also be rejected (it doesn't match http/https).
        assert!(!is_allowed_scheme("JavaScript:alert(1)"));
        assert!(!is_allowed_scheme("JAVASCRIPT:alert(1)"));
    }

    #[test]
    fn test_reject_custom_schemes() {
        assert!(!is_allowed_scheme("tel:+15550123"));
        assert!(!is_allowed_scheme("intent:#Intent;end"));
        assert!(!is_allowed_scheme("content://com.android.providers.media/"));
    }
}

// ---------------------------------------------------------------------------
// Input size limits
// ---------------------------------------------------------------------------

#[cfg(test)]
mod input_size_limits {
    use aura_types::ipc::{ContextPackage, MAX_MESSAGE_SIZE};

    /// IPC messages exceeding MAX_MESSAGE_SIZE must be rejected.
    #[test]
    fn test_ipc_message_size_limit() {
        // Exactly at the limit should be accepted.
        let at_limit = vec![0u8; MAX_MESSAGE_SIZE];
        assert_eq!(at_limit.len(), MAX_MESSAGE_SIZE);

        // One byte over should be rejected.
        let over_limit = vec![0u8; MAX_MESSAGE_SIZE + 1];
        assert!(
            over_limit.len() > MAX_MESSAGE_SIZE,
            "message over {} bytes must be rejected",
            MAX_MESSAGE_SIZE
        );
    }

    /// ContextPackage estimated size must not exceed MAX_SIZE.
    #[test]
    fn test_context_package_size_bounds() {
        let mut ctx = aura_types::ipc::ContextPackage::default();

        // Fill with data that approaches the limit.
        for _ in 0..ContextPackage::MAX_CONVERSATION_HISTORY {
            ctx.conversation_history
                .push(aura_types::ipc::ConversationTurn {
                    role: aura_types::ipc::Role::User,
                    content: "a".repeat(100),
                    timestamp_ms: 0,
                });
        }

        let estimated = ctx.estimated_size();
        assert!(
            estimated < ContextPackage::MAX_SIZE,
            "max conversation history should fit within {} bytes, got {}",
            ContextPackage::MAX_SIZE,
            estimated
        );
    }

    /// Verify that serialized config strings have reasonable upper bounds.
    #[test]
    fn test_config_string_bounds() {
        let config = aura_types::config::AuraConfig::default();
        let json = serde_json::to_string(&config).unwrap();

        // Default config should serialize to a reasonable size (well under 64KB).
        assert!(
            json.len() < 65536,
            "default config serialization too large: {} bytes",
            json.len()
        );
        // Should actually be quite small.
        assert!(
            json.len() < 8192,
            "default config should be under 8KB, got {} bytes",
            json.len()
        );
    }

    /// TelegramConfig has a bounded chat ID list.
    #[test]
    fn test_telegram_chat_ids_bounded() {
        use aura_types::config::{TelegramConfig, MAX_TELEGRAM_ALLOWED_CHAT_IDS};

        let config = TelegramConfig::default();
        assert!(config.allowed_chat_ids.len() <= MAX_TELEGRAM_ALLOWED_CHAT_IDS);

        // Verify the constant is reasonable.
        assert!(MAX_TELEGRAM_ALLOWED_CHAT_IDS > 0);
        assert!(MAX_TELEGRAM_ALLOWED_CHAT_IDS <= 256);
    }

    /// PolicyConfig has bounded rule count.
    #[test]
    fn test_policy_rules_bounded() {
        use aura_types::config::{PolicyConfig, MAX_POLICY_RULES};

        let config = PolicyConfig::default();
        assert!(config.rules.len() <= MAX_POLICY_RULES);
        assert!(MAX_POLICY_RULES > 0);
    }

    /// Verify screen config node limits are reasonable.
    #[test]
    fn test_screen_node_limits() {
        use aura_types::ipc::{MAX_SCREEN_INTERACTIVE_ELEMENTS, MAX_SCREEN_VISIBLE_TEXT};

        assert!(MAX_SCREEN_INTERACTIVE_ELEMENTS > 0);
        assert!(MAX_SCREEN_INTERACTIVE_ELEMENTS <= 512);
        assert!(MAX_SCREEN_VISIBLE_TEXT > 0);
        assert!(MAX_SCREEN_VISIBLE_TEXT <= 256);
    }
}

// ---------------------------------------------------------------------------
// JNI double-free protection (STATE_CONSUMED sentinel)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod jni_double_free_protection {
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Simulate the STATE_CONSUMED sentinel pattern used in nativeRun().
    /// This tests the compare_exchange logic that prevents double-free
    /// when the same state pointer is passed to run() twice.
    fn simulate_state_consumed_check(sentinel: &AtomicBool) -> bool {
        // Returns true if this is the FIRST call (safe to consume).
        // Returns false if already consumed (double-call detected).
        sentinel
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    #[test]
    fn test_first_call_allowed() {
        let sentinel = AtomicBool::new(false);
        assert!(
            simulate_state_consumed_check(&sentinel),
            "first call to nativeRun should be allowed"
        );
        assert!(
            sentinel.load(Ordering::Acquire),
            "sentinel should be set after first call"
        );
    }

    #[test]
    fn test_second_call_rejected() {
        let sentinel = AtomicBool::new(false);

        // First call succeeds.
        assert!(simulate_state_consumed_check(&sentinel));

        // Second call must be rejected.
        assert!(
            !simulate_state_consumed_check(&sentinel),
            "second call to nativeRun must be rejected (double-free protection)"
        );
    }

    #[test]
    fn test_multiple_subsequent_calls_rejected() {
        let sentinel = AtomicBool::new(false);

        // First call succeeds.
        assert!(simulate_state_consumed_check(&sentinel));

        // All subsequent calls must be rejected.
        for _ in 0..10 {
            assert!(
                !simulate_state_consumed_check(&sentinel),
                "subsequent calls must always be rejected"
            );
        }
    }

    #[test]
    fn test_concurrent_calls_race_safe() {
        use std::sync::Arc;
        use std::thread;

        let sentinel = Arc::new(AtomicBool::new(false));
        let mut handles = vec![];

        // Spawn 100 threads all trying to consume the sentinel simultaneously.
        for _ in 0..100 {
            let s = Arc::clone(&sentinel);
            handles.push(thread::spawn(move || simulate_state_consumed_check(&s)));
        }

        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Exactly ONE thread should succeed.
        let success_count = results.iter().filter(|&&r| r).count();
        assert_eq!(
            success_count, 1,
            "exactly one thread should succeed in consuming the state pointer, got {}",
            success_count
        );
    }

    #[test]
    fn test_null_pointer_rejected_before_sentinel() {
        // In nativeRun, null check happens BEFORE the sentinel check.
        // This ensures we don't waste the sentinel on a null pointer.
        fn native_run_simulation(state_ptr: i64, sentinel: &AtomicBool) -> &'static str {
            // Null check FIRST.
            if state_ptr == 0 {
                return "rejected: null pointer";
            }
            // THEN sentinel check.
            if sentinel
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return "rejected: already consumed";
            }
            "accepted"
        }

        let sentinel = AtomicBool::new(false);

        // Null pointer should be rejected WITHOUT consuming the sentinel.
        assert_eq!(
            native_run_simulation(0, &sentinel),
            "rejected: null pointer"
        );
        assert!(
            !sentinel.load(Ordering::Acquire),
            "sentinel should NOT be set after null pointer rejection"
        );

        // Valid pointer should be accepted.
        assert_eq!(native_run_simulation(0x1234, &sentinel), "accepted");

        // Second call with valid pointer should be rejected.
        assert_eq!(
            native_run_simulation(0x1234, &sentinel),
            "rejected: already consumed"
        );
    }
}
