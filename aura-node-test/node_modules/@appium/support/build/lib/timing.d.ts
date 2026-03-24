/** High-resolution start time: tuple from process.hrtime() or bigint from process.hrtime.bigint() */
type HrTime = [number, number] | bigint;
/**
 * Class representing a duration, encapsulating the number and units.
 */
export declare class Duration {
    private _nanos;
    constructor(_nanos: number);
    get nanos(): number;
    /**
     * Get the duration as nanoseconds
     *
     * @returns {number} The duration as nanoseconds
     */
    get asNanoSeconds(): number;
    /**
     * Get the duration converted into milliseconds
     *
     * @returns {number} The duration as milliseconds
     */
    get asMilliSeconds(): number;
    /**
     * Get the duration converted into seconds
     *
     * @returns {number} The duration fas seconds
     */
    get asSeconds(): number;
    toString(): string;
}
export declare class Timer {
    private _startTime;
    /**
     * Creates a timer
     */
    constructor(_startTime?: HrTime | null);
    get startTime(): HrTime | null;
    /**
     * Start the timer
     *
     * @return {Timer} The current instance, for chaining
     */
    start(): this;
    /**
     * Get the duration since the timer was started
     *
     * @return {Duration} the duration
     */
    getDuration(): Duration;
    toString(): string;
}
export { Timer as default };
//# sourceMappingURL=timing.d.ts.map