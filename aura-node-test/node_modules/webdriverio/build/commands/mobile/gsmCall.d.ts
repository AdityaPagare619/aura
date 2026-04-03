/**
 *
 * Make a GSM call on the Android emulator. Valid actions: 'call', 'accept', 'cancel', 'hold'.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :gsmCall.js
    it('should simulate a GSM call', async () => {
        // Simulate an incoming call
        await browser.gsmCall('+15551234567', 'call')
        // Accept the call
        await browser.gsmCall('+15551234567', 'accept')
    })
 * </example>
 *
 * @param {string}  phoneNumber  The phone number to use for the GSM call simulation
 * @param {string}  action       The action to perform ('call', 'accept', 'cancel', 'hold')
 *
 * @support ["android"]
 */
export declare function gsmCall(this: WebdriverIO.Browser, phoneNumber: string, action: string): Promise<unknown>;
//# sourceMappingURL=gsmCall.d.ts.map