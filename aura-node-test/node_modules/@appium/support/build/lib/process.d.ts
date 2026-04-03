/**
 * Get PIDs of processes whose executable name matches the given name (exact match via pgrep -x).
 *
 * @param appName - Executable name to match (e.g. 'tail', 'node').
 * @returns Promise resolving to an array of process IDs. Empty if no processes matched.
 * @throws {Error} If pgrep fails for any reason other than "no processes matched" (exit 1).
 * @deprecated Use a process-management API or package that fits your platform instead.
 */
export declare function getProcessIds(appName: string): Promise<number[]>;
/**
 * Kill all processes whose executable name matches the given name (via pkill -x).
 *
 * @param appName - Executable name to match (e.g. 'tail', 'node').
 * @param force - If true, use SIGKILL (-9); otherwise use default pkill signal.
 * @returns Promise that resolves when done, or when no matching processes were running.
 * @throws {Error} If pkill fails for any reason other than "no processes matched" (exit 1).
 * @deprecated Use a process-management API or package that fits your platform instead.
 */
export declare function killProcess(appName: string, force?: boolean): Promise<void>;
//# sourceMappingURL=process.d.ts.map