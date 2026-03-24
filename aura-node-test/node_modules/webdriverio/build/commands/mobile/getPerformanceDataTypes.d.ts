/**
 *
 * Returns the information types of the system state which is supported to read
 * as part of performance data (e.g. cpu, memory, network traffic, battery).
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :getPerformanceDataTypes.js
    it('should get the supported performance data types', async () => {
        const types = await browser.getPerformanceDataTypes()
        console.log('Supported types:', types)
    })
 * </example>
 *
 * @returns {`Promise<string[]>`} A list of supported performance data type names
 *
 * @support ["android"]
 */
export declare function getPerformanceDataTypes(this: WebdriverIO.Browser): Promise<string[]>;
//# sourceMappingURL=getPerformanceDataTypes.d.ts.map