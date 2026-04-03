/**
 * Whether the current OS is Windows.
 */
export declare function isWindows(): boolean;
/**
 * Whether the current OS is macOS (Darwin).
 */
export declare function isMac(): boolean;
/**
 * Whether the current OS is Linux (i.e. not Windows and not macOS).
 */
export declare function isLinux(): boolean;
/**
 * Whether the current Windows process is 64-bit (or WOW64).
 */
export declare function isOSWin64(): boolean;
/**
 * Detects the major.minor macOS version (e.g. "10.12") via `sw_vers -productVersion`.
 *
 * @returns The major.minor version string.
 * @throws {Error} If `sw_vers` fails or output cannot be parsed.
 */
export declare function macOsxVersion(): Promise<string>;
/**
 * System detection helpers (platform, architecture, macOS version).
 * Use this object when you need `arch()` to call other helpers via `this` (e.g. for testing).
 */
export declare const system: System;
/**
 * Resolves the process architecture as `'32'` or `'64'` (uname on Unix, process.arch/env on Windows).
 */
export declare const arch: () => Promise<string>;
interface System {
    isWindows(): boolean;
    isMac(): boolean;
    isLinux(): boolean;
    isOSWin64(): boolean;
    arch(): Promise<string>;
    macOsxVersion(): Promise<string>;
}
export {};
//# sourceMappingURL=system.d.ts.map