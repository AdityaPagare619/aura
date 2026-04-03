/**
 *
 * Simulate an SMS message being received on the Android emulator.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :sendSms.js
    it('should simulate receiving an SMS', async () => {
        await browser.sendSms('+15551234567', 'Hello from the test!')
    })
 * </example>
 *
 * @param {string}  phoneNumber  The phone number the simulated SMS is sent from
 * @param {string}  message      The text content of the simulated SMS
 *
 * @support ["android"]
 */
export declare function sendSms(this: WebdriverIO.Browser, phoneNumber: string, message: string): Promise<unknown>;
//# sourceMappingURL=sendSms.d.ts.map