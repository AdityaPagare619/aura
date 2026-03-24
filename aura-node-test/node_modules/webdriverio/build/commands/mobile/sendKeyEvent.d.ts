/**
 *
 * Send a key event to the Android device.
 *
 * This command is the legacy Android key event API. For new tests, prefer using
 * `pressKeyCode()` with numeric keycodes from the Android `KeyEvent` constants.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :sendKeyEvent.js
    it('should send a key event', async () => {
        // Send the Home key event (keycode '3')
        await browser.sendKeyEvent('3')
        // Send Shift+A (keycode '29', metastate '1')
        await browser.sendKeyEvent('29', '1')
    })
 * </example>
 *
 * @param {string}  keycode     The keycode to send (as a string, e.g. `'3'` for Home). See [Android KeyEvent](https://developer.android.com/reference/android/view/KeyEvent) for all available keycodes.
 * @param {string}  [metastate] The meta state to apply during the key press as a string (e.g. `'1'` for Shift). See [Android KeyEvent](https://developer.android.com/reference/android/view/KeyEvent) for all meta state values.
 *
 * @support ["android"]
 */
export declare function sendKeyEvent(this: WebdriverIO.Browser, keycode: string, metastate?: string): Promise<unknown>;
//# sourceMappingURL=sendKeyEvent.d.ts.map