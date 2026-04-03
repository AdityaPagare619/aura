/**
 * Utility function to extend node functionality, allowing us to require
 * modules that are installed globally. If the package cannot be required,
 * this will attempt to link the package and then re-require it
 *
 * @param packageName - the name of the package to be required
 * @returns The package object
 * @throws {Error} If the package is not found locally or globally
 */
export declare function requirePackage(packageName: string): Promise<unknown>;
/**
 * Calculate the in-depth size in memory of the provided object.
 * The original implementation is borrowed from https://github.com/miktam/sizeof.
 *
 * @param obj - An object whose size should be calculated
 * @returns Object size in bytes.
 */
export declare function getObjectSize(obj: unknown): number;
/**
 * Calculates a unique object identifier
 *
 * @param object - Any valid ECMA object
 * @returns A uuidV4 string that uniquely identifies given object
 */
export declare function getObjectId(object: object): string;
/**
 * Perform deep freeze of the given object (e. g.
 * all nested objects also become immutable).
 * If the passed object is of a plain type
 * then no change is done and the same object
 * is returned.
 * ! This function changes the given object,
 * so it becomes immutable.
 *
 * @param object - Any valid ECMA object
 * @returns The same object that was passed after it was made immutable.
 */
export declare function deepFreeze<T>(object: T): T;
/**
 * Tries to synchronously detect the absolute path to the folder
 * where the given `moduleName` is located.
 *
 * @param moduleName - The name of the module as it is written in package.json
 * @param filePath - Full path to any of files that `moduleName` contains. Use
 * `__filename` to find the root of the module where this helper is called.
 * @returns Full path to the module root, or null if not found
 */
export declare function getModuleRootSync(moduleName: string, filePath: string): string | null;
//# sourceMappingURL=node.d.ts.map