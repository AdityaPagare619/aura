/**
 *
 * Lock the device screen.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :lock.js
    it('should lock the screen', async () => {
        // Lock the screen indefinitely
        await browser.lock()
    })
 * </example>
 *
 * <example>
    :lock.timeout.js
    it('should lock the screen for 5 seconds (iOS only)', async () => {
        // Lock the screen for 5 seconds, then auto-unlock (iOS only)
        await browser.lock(5)
    })
 * </example>
 *
 * @param {number}  [seconds]   How long to lock the screen in seconds (iOS only)
 *
 * @support ["ios","android"]
 */
export declare function lock(this: WebdriverIO.Browser, seconds?: number): Promise<unknown>;
//# sourceMappingURL=lock.d.ts.map