/**
 *
 * Set the GSM signal strength on the Android emulator.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :gsmSignal.js
    it('should set GSM signal to full strength', async () => {
        await browser.gsmSignal(4)
    })
    it('should set GSM signal to no signal', async () => {
        await browser.gsmSignal(0)
    })
 * </example>
 *
 * @param {number}  signalStrength  The signal strength to set. Accepted values: `0` (no signal), `1` (very poor), `2` (poor), `3` (moderate), `4` (good/full)
 *
 * @support ["android"]
 */
export declare function gsmSignal(this: WebdriverIO.Browser, signalStrength: number): Promise<unknown>;
//# sourceMappingURL=gsmSignal.d.ts.map