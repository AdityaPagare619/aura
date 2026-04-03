/**
 *
 * Simulate a Touch ID or Face ID biometric match event on iOS Simulator.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * :::note
 * This command requires the `allowTouchIdEnroll` capability to be set to `true` in the session.
 * It is only supported on iOS Simulator.
 * :::
 *
 * <example>
    :touchId.js
    it('should simulate a Touch ID fingerprint match', async () => {
        // Simulate a successful Touch ID match
        await browser.touchId(true)
        // Simulate a failed Touch ID match
        await browser.touchId(false)
    })
    it('should simulate a Face ID match', async () => {
        // Simulate a successful Face ID match
        await browser.touchId(true, 'faceId')
        // Simulate a failed Face ID match
        await browser.touchId(false, 'faceId')
    })
 * </example>
 *
 * @param {boolean} match   Whether the biometric match should succeed (`true`) or fail (`false`)
 * @param {string}  [type]  The biometric type to simulate. Use `'touchId'` for Touch ID (default) or `'faceId'` for Face ID. <br /><strong>iOS-ONLY</strong>
 *
 * @support ["ios"]
 */
export declare function touchId(this: WebdriverIO.Browser, match: boolean, type?: 'touchId' | 'faceId'): Promise<unknown>;
//# sourceMappingURL=touchId.d.ts.map