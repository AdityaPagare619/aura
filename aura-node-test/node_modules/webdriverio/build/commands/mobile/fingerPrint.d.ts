/**
 *
 * Authenticate users by using their fingerprint scan on supported Android emulators. The
 * fingerprintId must be between 1 and 10.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * <example>
    :fingerPrint.js
    it('should authenticate with fingerprint', async () => {
        await browser.fingerPrint(1)
    })
 * </example>
 *
 * @param {number}  fingerprintId  The fingerprint sensor ID to simulate (1–10)
 *
 * @support ["android"]
 */
export declare function fingerPrint(this: WebdriverIO.Browser, fingerprintId: number): Promise<unknown>;
//# sourceMappingURL=fingerPrint.d.ts.map