/**
 * Preprocesses the resulting value for API responses,
 * so they have keys for both W3C and JSONWP protocols.
 * The argument value is NOT mutated.
 *
 * @param resValue - The actual response value
 * @returns Either modified value or the same one if nothing has been modified
 */
export declare function formatResponseValue(resValue: object | undefined): object | null;
/**
 * Properly formats the status for API responses,
 * so they are correct for the W3C protocol.
 *
 * @param responseBody - The response body
 * @returns The fixed response body
 */
export declare function ensureW3cResponse(responseBody: Record<string, unknown>): Record<string, unknown>;
//# sourceMappingURL=helpers.d.ts.map