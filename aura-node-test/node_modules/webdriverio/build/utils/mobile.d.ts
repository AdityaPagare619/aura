import type { PinchAndZoomOptions } from '../types.js';
/**
 * Returns true if the error indicates that the driver does not know about the
 * requested `mobile:` execute method (old Appium 2 driver). Any other error
 * (wrong params, device disconnected, etc.) should be re-thrown by the caller.
 */
export declare function isUnknownMethodError(err: unknown): boolean;
/**
 * Log a deprecation warning when a mobile command falls back to the legacy
 * Appium protocol endpoint because the driver is too old to support the
 * modern `mobile:` execute replacement.
 *
 * @param mobileCommand   e.g. `'mobile: lock'`
 * @param protocolEndpoint  e.g. `'/appium/device/lock'`
 */
export declare function logAppiumDeprecationWarning(mobileCommand: string, protocolEndpoint: string): void;
export declare function getNativeContext({ capabilities, isMobile }: {
    capabilities: WebdriverIO.Capabilities;
    isMobile: boolean;
}): boolean;
export declare function getMobileContext({ capabilities, isAndroid, isNativeContext }: {
    capabilities: WebdriverIO.Capabilities;
    isAndroid: boolean;
    isNativeContext: boolean;
}): string | undefined;
export declare function calculateAndroidPinchAndZoomSpeed({ browser, duration, scale }: {
    browser: WebdriverIO.Browser;
    duration: number;
    scale: number;
}): number;
export declare function validatePinchAndZoomOptions({ browser, gesture, options }: {
    browser: WebdriverIO.Browser;
    gesture: 'pinch' | 'zoom';
    options: Partial<PinchAndZoomOptions>;
}): {
    scale: number;
    duration: number;
};
//# sourceMappingURL=mobile.d.ts.map