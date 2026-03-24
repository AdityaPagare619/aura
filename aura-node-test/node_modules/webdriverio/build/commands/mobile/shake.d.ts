/**
 *
 * Perform a shake action on the device. Supports iOS Simulator and real devices.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :shake.js
    it('should shake the device', async () => {
        await browser.shake()
    })
 * </example>
 *
 * @support ["ios"]
 */
export declare function shake(this: WebdriverIO.Browser): Promise<unknown>;
//# sourceMappingURL=shake.d.ts.map