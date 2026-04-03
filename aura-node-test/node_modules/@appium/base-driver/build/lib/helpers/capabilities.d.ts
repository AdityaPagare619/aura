import type { Constraints, W3CCapabilities, Capabilities, AppiumLogger } from '@appium/types';
/**
 * Determine whether the given argument is valid
 * W3C capabilities instance.
 */
export declare function isW3cCaps(caps: unknown): caps is W3CCapabilities<Constraints>;
/**
 * Normalize capability values according to constraints (e.g. string 'true' → boolean).
 */
export declare function fixCaps<C extends Constraints>(oldCaps: Record<string, unknown>, desiredCapConstraints: C, log: AppiumLogger): Capabilities<C>;
//# sourceMappingURL=capabilities.d.ts.map