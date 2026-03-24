/**
 *
 * Check whether the device screen is locked.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :isLocked.js
    it('should check if the device is locked', async () => {
        const locked = await browser.isLocked()
        console.log('Device is locked:', locked)
    })
 * </example>
 *
 * @returns {`Promise<boolean>`} `true` if the device is locked, `false` otherwise
 *
 * @support ["ios","android"]
 */
export declare function isLocked(this: WebdriverIO.Browser): Promise<boolean>;
//# sourceMappingURL=isLocked.d.ts.map