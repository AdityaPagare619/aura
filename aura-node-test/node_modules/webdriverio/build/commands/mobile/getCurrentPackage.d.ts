/**
 *
 * Get the name of the current Android package.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :getCurrentPackage.js
    it('should get the current Android package', async () => {
        const pkg = await browser.getCurrentPackage()
        console.log('Current package:', pkg)
    })
 * </example>
 *
 * @returns {`Promise<string>`} The name of the current Android package
 *
 * @support ["android"]
 */
export declare function getCurrentPackage(this: WebdriverIO.Browser): Promise<string>;
//# sourceMappingURL=getCurrentPackage.d.ts.map