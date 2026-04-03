/**
 *
 * Get the display density from the device.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :getDisplayDensity.js
    it('should get the display density', async () => {
        const density = await browser.getDisplayDensity()
        console.log('Display density:', density)
    })
 * </example>
 *
 * @support ["android"]
 */
export declare function getDisplayDensity(this: WebdriverIO.Browser): Promise<any>;
//# sourceMappingURL=getDisplayDensity.d.ts.map