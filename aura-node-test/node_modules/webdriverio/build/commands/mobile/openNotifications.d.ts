/**
 *
 * Open Android notifications.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :openNotifications.js
    it('should open the Android notification shade', async () => {
        await browser.openNotifications()
    })
 * </example>
 *
 * @support ["android"]
 */
export declare function openNotifications(this: WebdriverIO.Browser): Promise<unknown>;
//# sourceMappingURL=openNotifications.d.ts.map