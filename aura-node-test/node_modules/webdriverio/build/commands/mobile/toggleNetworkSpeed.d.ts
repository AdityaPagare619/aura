/**
 *
 * Set the network speed for the Android emulator. Valid values: 'full', 'gsm', 'edge', 'hscsd',
 * 'gprs', 'umts', 'hsdpa', 'lte', 'evdo'.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :toggleNetworkSpeed.js
    it('should set network speed to LTE', async () => {
        await browser.toggleNetworkSpeed('lte')
    })
 * </example>
 *
 * @param {string}  netspeed  The network speed preset to apply ('full', 'gsm', 'edge', 'hscsd', 'gprs', 'umts', 'hsdpa', 'lte', 'evdo')
 *
 * @support ["android"]
 */
export declare function toggleNetworkSpeed(this: WebdriverIO.Browser, netspeed: string): Promise<unknown>;
//# sourceMappingURL=toggleNetworkSpeed.d.ts.map