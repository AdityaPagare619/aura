/**
 *
 * Toggle the state of the location service.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :toggleLocationServices.js
    it('should toggle location services', async () => {
        await browser.toggleLocationServices()
    })
 * </example>
 *
 * @support ["android"]
 */
export declare function toggleLocationServices(this: WebdriverIO.Browser): Promise<unknown>;
//# sourceMappingURL=toggleLocationServices.d.ts.map