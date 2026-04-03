/**
 *
 * Set the state of the battery charger on the Android emulator. Valid values: 'on', 'off'.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :powerAC.js
    it('should enable the AC charger', async () => {
        await browser.powerAC('on')
    })
 * </example>
 *
 * @param {string}  state  The charger state to set ('on' or 'off')
 *
 * @support ["android"]
 */
export declare function powerAC(this: WebdriverIO.Browser, state: string): Promise<unknown>;
//# sourceMappingURL=powerAC.d.ts.map