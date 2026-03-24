/**
 *
 * Set the airplane mode state on the device.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * :::note
 * Unlike the deprecated API which toggled the airplane mode state, this command requires an
 * explicit `enabled` parameter to set the desired state directly.
 *
 * This command is only supported on Android.
 * :::
 *
 * <example>
    :toggleAirplaneMode.js
    it('should set airplane mode', async () => {
        // Enable airplane mode
        await browser.toggleAirplaneMode(true)
        // Disable airplane mode
        await browser.toggleAirplaneMode(false)
    })
 * </example>
 *
 * @param {boolean} enabled   Set to `true` to enable airplane mode, `false` to disable it.
 *
 * @support ["android"]
 */
export declare function toggleAirplaneMode(this: WebdriverIO.Browser, enabled: boolean): Promise<unknown>;
//# sourceMappingURL=toggleAirplaneMode.d.ts.map