//! Regression test for F003: Termux permission denied
//!
//! Verifies that AURA has correct permissions and HOME is set.

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::Path;

    /// Verify HOME is set
    #[test]
    fn test_home_environment_set() {
        let home = env::var("HOME").or_else(|_| env::var("USERPROFILE"));

        assert!(
            home.is_ok(),
            "HOME environment variable must be set! \
             On Termux: export HOME=/data/data/com.termux/files/home"
        );

        let home_path = Path::new(home.as_ref().unwrap());
        assert!(
            home_path.exists(),
            "HOME directory {} must exist",
            home_path.display()
        );
    }

    /// Verify we can write to HOME
    #[test]
    fn test_home_writable() {
        let home = env::var("HOME")
            .or_else(|_| env::var("USERPROFILE"))
            .expect("HOME must be set");

        let test_file = Path::new(&home).join(".aura_test_write");

        // Clean up any existing file
        let _ = std::fs::remove_file(&test_file);

        std::fs::write(&test_file, "test").expect(
            "HOME directory must be writable! \
             On Termux: termux-setup-storage to grant storage permissions",
        );

        std::fs::remove_file(&test_file).ok();
    }
}
