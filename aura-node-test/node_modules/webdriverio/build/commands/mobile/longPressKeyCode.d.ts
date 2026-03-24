/**
 *
 * Press and hold a particular key code on the device.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :longPressKeyCode.js
    it('should long press the Home button', async () => {
        await browser.longPressKeyCode(3)
    })
 * </example>
 *
 * @param {number}  keycode      The keycode to long-press (Android KeyEvent constant)
 * @param {number}  [metastate]  Meta state to apply during the key press (e.g. shift, ctrl)
 * @param {number}  [flags]      Integer flags for the key event
 *
 * @support ["android"]
 */
export declare function longPressKeyCode(this: WebdriverIO.Browser, keycode: number, metastate?: number, flags?: number): Promise<unknown>;
//# sourceMappingURL=longPressKeyCode.d.ts.map