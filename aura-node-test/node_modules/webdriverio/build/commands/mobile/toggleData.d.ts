/**
 *
 * Set the mobile data state on the device.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * :::note
 * Unlike the deprecated API which toggled the data state, this command requires an explicit
 * `enabled` parameter to set the desired state directly.
 *
 * This command is only supported on Android.
 * :::
 *
 * <example>
    :toggleData.js
    it('should set mobile data state', async () => {
        await browser.toggleData(true)  // enable data
        await browser.toggleData(false) // disable data
    })
 * </example>
 *
 * @param {boolean} enabled   Set to `true` to enable mobile data, `false` to disable it.
 *
 * @support ["android"]
 */
export declare function toggleData(this: WebdriverIO.Browser, enabled: boolean): Promise<unknown>;
//# sourceMappingURL=toggleData.d.ts.map