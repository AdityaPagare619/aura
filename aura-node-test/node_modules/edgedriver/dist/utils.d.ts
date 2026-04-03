import type { EdgedriverParameters } from './types.js';
interface Priorities {
    regex: RegExp;
    weight: number;
}
export declare function getNameByArchitecture(): string;
export declare function parseParams(params: EdgedriverParameters): string[];
/**
 * helper utility to clone a list
 * @param  {Any[]} arr  list of things
 * @return {Any[]}      new list of same things
 */
export declare function uniq(arr: string[]): string[];
export declare function sort(installations: string[], priorities: Priorities[]): string[];
/**
 * Look for edge executables by using the which command
 */
export declare function findByWhich(executables: string[], priorities: Priorities[]): string[];
/**
 * Helper utility to check file access
 * @param {string} file file to check access for
 * @return              true if file can be accessed
 */
export declare function hasAccessSync(filePath: string): boolean;
export declare function hasAccess(filePath: string): Promise<boolean>;
export declare function sleep(ms?: number): Promise<unknown>;
export interface BasicAuthResult {
    url: string;
    authHeader?: string;
}
/**
 * Extract Basic Auth credentials from a URL and return the cleaned URL with auth header.
 * This is needed because fetch() doesn't support URLs with embedded credentials.
 * @param urlString URL that may contain credentials (e.g., https://user:pass@host/)
 * @returns Object with cleaned URL and optional Authorization header
 */
export declare function extractBasicAuthFromUrl(urlString: string): BasicAuthResult;
export {};
//# sourceMappingURL=utils.d.ts.map