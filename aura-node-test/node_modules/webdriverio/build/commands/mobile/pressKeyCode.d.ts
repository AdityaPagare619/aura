/**
 *
 * Press a particular key on the device. The keycode values correspond to Android KeyEvent constants.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :pressKeyCode.js
    it('should press the Home button', async () => {
        // Press the Home button (keycode 3)
        await browser.pressKeyCode(3)
        // Press with meta state (e.g., Shift + A = keycode 29, metastate 1)
        await browser.pressKeyCode(29, 1)
    })
 * </example>
 *
 * @param {number}  keycode      The keycode to press (Android KeyEvent constant)
 * @param {number}  [metastate]  Meta state to apply during the key press (e.g. shift, ctrl)
 * @param {number}  [flags]      Integer flags for the key event
 *
 * @support ["android"]
 */
export declare function pressKeyCode(this: WebdriverIO.Browser, keycode: number, metastate?: number, flags?: number): Promise<unknown>;
//# sourceMappingURL=pressKeyCode.d.ts.map