/**
 *
 * Get the name of the current Android activity.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :getCurrentActivity.js
    it('should get the current Android activity', async () => {
        const activity = await browser.getCurrentActivity()
        console.log('Current activity:', activity)
    })
 * </example>
 *
 * @returns {`Promise<string>`} The name of the current Android activity
 *
 * @support ["android"]
 */
export declare function getCurrentActivity(this: WebdriverIO.Browser): Promise<string>;
//# sourceMappingURL=getCurrentActivity.d.ts.map