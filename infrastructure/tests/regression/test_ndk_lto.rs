//! Regression test for F001: SIGSEGV at startup with NDK r26b
//!
//! This test ensures the lto=true + panic=abort combination
//! that causes SIGSEGV on NDK r26b never reoccurs.
//!
//! Run with: cargo test --test test_ndk_lto

use std::fs;
use std::path::Path;

/// Verifies Cargo.toml does NOT have the problematic lto=true + panic=abort combo
#[test]
fn test_no_lto_true_with_panic_abort() {
    let cargo_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("Cargo.toml");

    let content = fs::read_to_string(&cargo_path).expect("Failed to read Cargo.toml");

    let has_lto_true = content.contains(r#"lto = "true""#) || content.contains("lto=true");
    let has_panic_abort =
        content.contains(r#"panic = "abort""#) || content.contains("panic=\"abort\"");

    // Either is fine individually, but NOT together
    if has_lto_true && has_panic_abort {
        panic!(
            "DANGER: Cargo.toml has lto=true AND panic=abort!\n\
             This combination causes SIGSEGV on NDK r26b.\n\
             Fix: Change to lto=\"thin\" or panic=\"unwind\""
        );
    }

    // Ideally use lto="thin" for Android builds
    if has_lto_true && !has_panic_abort {
        println!("WARNING: lto=true is set. Consider lto=\"thin\" for better compatibility.");
    }
}

/// Verifies the fix is in place
#[test]
fn test_lto_fix_applied() {
    let cargo_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("Cargo.toml");

    let content = fs::read_to_string(&cargo_path).expect("Failed to read Cargo.toml");

    // The fix should use lto="thin" or lto=false, NOT lto=true
    if content.contains(r#"lto = "true""#) {
        panic!(
            "Cargo.toml still uses lto=true!\n\
             F001 Fix requires: lto=\"thin\" in Cargo.toml"
        );
    }

    // Verify panic is not abort
    if content.contains(r#"panic = "abort""#) {
        panic!(
            "Cargo.toml still uses panic=abort!\n\
             F001 Fix requires: panic=\"unwind\" in Cargo.toml"
        );
    }

    println!("F001 Regression Test PASSED: No lto=true + panic=abort combination");
}
