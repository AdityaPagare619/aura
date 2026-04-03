import type { Constraint } from '@appium/types';
export declare class Validator {
    private readonly _validators;
    validate(values: Record<string, any>, constraints: Record<string, Constraint>): Record<string, string[]> | null;
}
export declare const validator: Validator;
//# sourceMappingURL=validation.d.ts.map