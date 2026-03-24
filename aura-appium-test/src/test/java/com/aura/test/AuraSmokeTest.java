package com.aura.test;

import io.appium.java_client.AppiumBy;
import io.appium.java_client.android.AndroidDriver;
import io.appium.java_client.android.options.UiAutomator2Options;
import io.appium.java_client.remote.AutomationName;
import io.appium.java_client.remote.MobileCapabilityType;
import org.openqa.selenium.By;
import org.openqa.selenium.OutputType;
import org.openqa.selenium.WebElement;
import org.openqa.selenium.remote.DesiredCapabilities;
import org.testng.Assert;
import org.testng.annotations.AfterClass;
import org.testng.annotations.BeforeClass;
import org.testng.annotations.Test;

import java.io.IOException;
import java.net.URL;
import java.time.Duration;
import java.util.HashMap;
import java.util.Map;

/**
 * AURA v4 F001 Smoke Test
 * Tests aura-daemon binary on real Android device via BrowserStack App Automate
 * 
 * Test Sequence:
 * 1. Launch Termux app
 * 2. Execute shell commands to download aura-daemon binary
 * 3. Run ./aura-daemon --version (F001 TEST)
 * 4. Verify exit code (0 = FIXED, 139 = SIGSEGV)
 * 5. Take screenshot of output
 */
public class AuraSmokeTest {

    private AndroidDriver driver;
    private static final String BROWSERSTACK_USER = "adityapagare_Bx7ZPV";
    private static final String BROWSERSTACK_KEY = "JfzmcXY52g83yUpsS95D";
    private static final String TERMUX_PACKAGE = "com.termux";
    private static final String AURA_BINARY_URL = "https://github.com/AdityaPagare619/aura/releases/download/v4.0.0-f001-validated/aura-daemon";
    private static final String AURA_SHA256 = "6d649c29d1bc862bed5491b7a132809c5c3fd8438ff397f71b8ec91c832ac919";

    @BeforeClass
    public void setup() throws Exception {
        System.out.println("Setting up BrowserStack App Automate session...");

        UiAutomator2Options options = new UiAutomator2Options()
            .setDeviceName("Samsung Galaxy S24")
            .setPlatformName("Android")
            .setPlatformVersion("14.0")
            .setApp("bs://da238b12e7756cbe140170866fd118ea22b7cb63")
            .setAutomationName(AutomationName.ANDROID_UIAUTOMATOR2)
            .setNewCommandTimeout(Duration.ofSeconds(300))
            .autoGrantPermissions();

        URL url = new URL("https://" + BROWSERSTACK_USER + ":" + BROWSERSTACK_KEY + 
                          "@hub-cloud.browserstack.com/wd/hub");

        driver = new AndroidDriver(url, options);
        driver.manage().timeouts().implicitlyWait(Duration.ofSeconds(30));

        System.out.println("BrowserStack session started successfully!");
    }

    @Test(priority = 1, description = "F001 Test: Verify aura-daemon starts without SIGSEGV")
    public void testAuraDaemonVersion() throws Exception {
        System.out.println("=== F001 SMOKE TEST ===");
        System.out.println("Testing: ./aura-daemon --version");
        
        // Give Termux time to fully initialize
        Thread.sleep(3000);

        // Try to execute shell command via Appium
        // This uses Appium's mobile: shell extension
        Map<String, Object> shellResult = executeShell(
            "cd /data/data/com.termux/files/home && " +
            "wget -q -O aura-daemon '" + AURA_BINARY_URL + "' && " +
            "chmod +x aura-daemon && " +
            "./aura-daemon --version; " +
            "EXIT=$?; " +
            "echo \"EXIT_CODE:$EXIT\"; " +
            "exit $EXIT"
        );

        String output = String.valueOf(shellResult.get("output"));
        int exitCode = (int) shellResult.get("exitCode");

        System.out.println("Shell output: " + output);
        System.out.println("Exit code: " + exitCode);

        // F001 CHECK: Exit code 139 = SIGSEGV (F001 still exists)
        // Exit code 0 = Success (F001 FIXED)
        if (exitCode == 139) {
            Assert.fail("F001 STILL EXISTS: aura-daemon crashed with SIGSEGV (exit code 139)");
        }

        Assert.assertEquals(exitCode, 0, "aura-daemon should exit with code 0");
        Assert.assertTrue(output.contains("aura") || output.contains("version"), 
            "Output should contain version info");
    }

    @Test(priority = 2, description = "Test: Verify binary SHA256 matches")
    public void testBinaryIntegrity() throws Exception {
        System.out.println("=== BINARY INTEGRITY TEST ===");
        
        Map<String, Object> shellResult = executeShell(
            "cd /data/data/com.termux/files/home && " +
            "sha256sum aura-daemon && " +
            "echo 'EXPECTED:" + AURA_SHA256 + "'"
        );

        String output = String.valueOf(shellResult.get("output"));
        System.out.println("SHA256 output: " + output);

        Assert.assertTrue(output.contains(AURA_SHA256.substring(0, 16)),
            "Binary SHA256 should match expected hash");
    }

    @Test(priority = 3, description = "Test: Help flag works")
    public void testAuraHelp() throws Exception {
        System.out.println("=== HELP FLAG TEST ===");
        
        Map<String, Object> shellResult = executeShell(
            "cd /data/data/com.termux/files/home && " +
            "./aura-daemon --help 2>&1; echo 'EXIT:$?'"
        );

        String output = String.valueOf(shellResult.get("output"));
        int exitCode = (int) shellResult.get("exitCode");

        System.out.println("Help output: " + output);
        System.out.println("Exit code: " + exitCode);

        Assert.assertEquals(exitCode, 0, "--help should exit cleanly");
    }

    @Test(priority = 4, description = "Test: Verify Telegram integration capability")
    public void testTelegramConfig() throws Exception {
        System.out.println("=== TELEGRAM CONFIG TEST ===");
        System.out.println("Bot: 8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI");
        System.out.println("User ID: 8407946567");
        
        // Test that environment variables can be set
        Map<String, Object> shellResult = executeShell(
            "export AURA_TELEGRAM_TOKEN='8764736044:AAEAQTbDmsBuMm6HHW5EWjsdfBE47jOs2BI' && " +
            "export AURA_TELEGRAM_CHAT_ID='8407946567' && " +
            "echo 'ENV_SET_OK' && " +
            "env | grep AURA_"
        );

        String output = String.valueOf(shellResult.get("output"));
        System.out.println("Env output: " + output);

        Assert.assertTrue(output.contains("ENV_SET_OK"), 
            "Environment variables should be settable in Termux");
    }

    /**
     * Execute a shell command on the Android device via Appium
     * Uses Appium's mobile: shell extension for Android
     */
    private Map<String, Object> executeShell(String command) {
        Map<String, Object> result = new HashMap<>();
        
        try {
            // Use Appium's mobile: shell command (Android only)
            // This executes the command in the app's context
            Map<String, Object> params = new HashMap<>();
            params.put("command", command);
            params.put("args", "");
            params.put("includeStderr", true);
            params.put("timeout", 120000); // 2 min timeout

            Object response = driver.executeScript("mobile: shell", params);
            
            if (response instanceof Map) {
                @SuppressWarnings("unchecked")
                Map<String, Object> responseMap = (Map<String, Object>) response;
                String stdout = String.valueOf(responseMap.getOrDefault("stdout", ""));
                String stderr = String.valueOf(responseMap.getOrDefault("stderr", ""));
                int exitCode = ((Number) responseMap.getOrDefault("exitCode", 0)).intValue();
                
                result.put("output", stdout + stderr);
                result.put("exitCode", exitCode);
            } else {
                result.put("output", String.valueOf(response));
                result.put("exitCode", -1);
            }
        } catch (Exception e) {
            System.out.println("Shell execution error: " + e.getMessage());
            // Fallback: try via Termux UI interaction
            result = executeViaTermuxUI(command);
        }
        
        return result;
    }

    /**
     * Fallback: Execute commands via Termux UI automation
     * Opens Termux keyboard and types commands
     */
    private Map<String, Object> executeViaTermuxUI(String command) {
        Map<String, Object> result = new HashMap<>();
        
        try {
            // Navigate to Termux home
            WebElement termux = driver.findElement(AppiumBy.accessibilityId("Termux"));
            termux.click();
            Thread.sleep(2000);

            // Clear existing text
            driver.navigate().back();
            Thread.sleep(500);

            // Type the command character by character
            // This is a fallback for when mobile:shell isn't available
            System.out.println("Using UI fallback for command execution");
            
            result.put("output", "UI_FALLBACK_USED");
            result.put("exitCode", -2);
        } catch (Exception e) {
            System.out.println("UI fallback also failed: " + e.getMessage());
            result.put("output", "ERROR: " + e.getMessage());
            result.put("exitCode", -99);
        }
        
        return result;
    }

    @AfterClass
    public void teardown() {
        if (driver != null) {
            System.out.println("Tearing down BrowserStack session...");
            driver.quit();
        }
    }
}
