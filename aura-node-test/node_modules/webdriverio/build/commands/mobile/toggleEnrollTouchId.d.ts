/**
 *
 * Toggle Touch ID enrollment on iOS Simulator.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * :::note
 * This command requires the `allowTouchIdEnroll` capability to be set to `true` in the session.
 * It is only supported on iOS Simulator. When `enabled` is not provided, it defaults to `true`.
 * :::
 *
 * <example>
    :toggleEnrollTouchId.js
    it('should toggle Touch ID enrollment', async () => {
        // Enable Touch ID enrollment
        await browser.toggleEnrollTouchId(true)
        // Disable Touch ID enrollment
        await browser.toggleEnrollTouchId(false)
    })
 * </example>
 *
 * @param {boolean} [enabled]   Whether to enable (`true`) or disable (`false`) Touch ID enrollment. Defaults to `true`.
 *
 * @support ["ios"]
 */
export declare function toggleEnrollTouchId(this: WebdriverIO.Browser, enabled?: boolean): Promise<unknown>;
//# sourceMappingURL=toggleEnrollTouchId.d.ts.map