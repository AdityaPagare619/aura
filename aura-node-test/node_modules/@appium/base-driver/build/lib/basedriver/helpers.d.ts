import type { ConfigureAppOptions } from '@appium/types';
export declare const BASEDRIVER_VER: string;
/**
 * Performs initial application package configuration so the app is ready for driver use.
 * Resolves local paths, downloads remote apps (http/https) with optional caching, and
 * runs optional post-process or custom download hooks.
 *
 * @param app - Path to a local app or URL of a downloadable app (http/https).
 * @param options - Supported extensions and optional hooks. Either a single extension
 * string, an array of extension strings, or {@link ConfigureAppOptions} (e.g.
 * `supportedExtensions`, `onPostProcess`, `onDownload`).
 * @returns Resolved path to the application (local path or path to downloaded/cached app).
 * @throws {Error} If supported extensions are missing, the app path/URL is invalid, or download fails.
 */
export declare function configureApp(app: string, options?: string | string[] | ConfigureAppOptions): Promise<string>;
/**
 * Returns whether the given string looks like a package or bundle identifier
 * (e.g. `com.example.app` or `org.company.AnotherApp`).
 *
 * @param app - Value to check (e.g. app path or bundle id).
 * @returns `true` if the value matches a dot-separated identifier pattern.
 */
export declare function isPackageOrBundle(app: string): boolean;
/**
 * Recursively ensures both keys exist with the same value in objects and arrays.
 * For each object, if `firstKey` exists its value is also set at `secondKey`, and vice versa.
 *
 * @param input - Object, array, or primitive to process (arrays/objects traversed recursively).
 * @param firstKey - First key name to mirror.
 * @param secondKey - Second key name to mirror.
 * @returns A deep copy of `input` with both keys present where objects had either key.
 */
export declare function duplicateKeys<T>(input: T, firstKey: string, secondKey: string): T;
/**
 * Normalizes a capability value to a string array. If already an array, returns it;
 * if a string, parses as JSON array when possible, otherwise returns a single-element array.
 *
 * @param capValue - Capability value: string (including JSON array like `"[\"a\",\"b\"]"`) or string[].
 * @returns Array of strings.
 * @throws {TypeError} If value is not a string/array or JSON parsing fails for array-like input.
 */
export declare function parseCapsArray(capValue: string | string[]): string[];
/**
 * Builds a short log prefix for a driver instance (e.g. `UiAutomator2@a1b2`).
 *
 * @param obj - Driver or other object; its constructor name and a short id are used.
 * @param _sessionId - Deprecated and unused; kept for {@link DriverHelpers} interface compatibility.
 * @returns Prefix string like `DriverName@xxxx`, or `UnknownDriver@????` if `obj` is null.
 */
export declare function generateDriverLogPrefix(obj: object | null, _sessionId?: string | null): string;
declare const _default: {
    configureApp: typeof configureApp;
    isPackageOrBundle: typeof isPackageOrBundle;
    duplicateKeys: typeof duplicateKeys;
    parseCapsArray: typeof parseCapsArray;
    generateDriverLogPrefix: typeof generateDriverLogPrefix;
};
export default _default;
//# sourceMappingURL=helpers.d.ts.map