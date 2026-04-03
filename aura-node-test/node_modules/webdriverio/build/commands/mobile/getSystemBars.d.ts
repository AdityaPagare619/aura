/**
 *
 * Retrieve visibility and bounds information of the status and navigation bars.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :getSystemBars.js
    it('should get system bars info', async () => {
        const bars = await browser.getSystemBars()
        console.log('Status bar:', bars.statusBar)
        console.log('Navigation bar:', bars.navigationBar)
    })
 * </example>
 *
 * @support ["android"]
 */
export declare function getSystemBars(this: WebdriverIO.Browser): Promise<unknown>;
//# sourceMappingURL=getSystemBars.d.ts.map