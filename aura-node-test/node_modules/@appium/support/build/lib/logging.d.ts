import type { AppiumLogger, AppiumLoggerLevel, AppiumLoggerPrefix } from '@appium/types';
export declare const LEVELS: readonly AppiumLoggerLevel[];
export declare const log: AppiumLogger;
/**
 * @param prefix - Optional log prefix
 * @returns A wrapped Appium logger instance
 */
export declare function getLogger(prefix?: AppiumLoggerPrefix | null): AppiumLogger;
/**
 * Marks arbitrary log message as sensitive.
 * This message will then be replaced with the default replacer
 * while being logged by any `info`, `debug`, etc. methods if the
 * asyncStorage has `isSensitive` flag enabled in its async context.
 * The latter is enabled by the corresponding HTTP middleware
 * in response to the `X-Appium-Is-Sensitive` request header
 * being set to 'true'.
 */
export declare function markSensitive<T>(logMessage: T): {
    [k: string]: T;
};
export default log;
//# sourceMappingURL=logging.d.ts.map