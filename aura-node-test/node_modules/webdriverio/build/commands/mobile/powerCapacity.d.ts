/**
 *
 * Set the battery percentage on the Android emulator. Value must be in the range [0, 100].
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :powerCapacity.js
    it('should set battery to 75%', async () => {
        await browser.powerCapacity(75)
    })
 * </example>
 *
 * @param {number}  percent  The battery percentage to set (0–100)
 *
 * @support ["android"]
 */
export declare function powerCapacity(this: WebdriverIO.Browser, percent: number): Promise<unknown>;
//# sourceMappingURL=powerCapacity.d.ts.map