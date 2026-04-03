/**
 *
 * Set the GSM voice state on the Android emulator. Valid values: 'unregistered', 'home',
 * 'roaming', 'searching', 'denied', 'off', 'on'.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :gsmVoice.js
    it('should set GSM voice state to home', async () => {
        await browser.gsmVoice('home')
    })
 * </example>
 *
 * @param {string}  state  The GSM voice state to set ('unregistered', 'home', 'roaming', 'searching', 'denied', 'off', 'on')
 *
 * @support ["android"]
 */
export declare function gsmVoice(this: WebdriverIO.Browser, state: string): Promise<unknown>;
//# sourceMappingURL=gsmVoice.d.ts.map