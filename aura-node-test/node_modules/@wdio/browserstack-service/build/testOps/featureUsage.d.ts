declare class FeatureUsage {
    private isTriggered?;
    private status?;
    private error?;
    constructor(isTriggered?: boolean);
    getTriggered(): boolean | undefined;
    setTriggered(triggered: boolean): void;
    setStatus(status: string): void;
    setError(error: string): void;
    triggered(): void;
    failed(e: unknown): void;
    success(): void;
    getStatus(): string | undefined;
    getError(): string | undefined;
    toJSON(): {
        isTriggered: boolean | undefined;
        status: string | undefined;
        error: string | undefined;
    };
}
export default FeatureUsage;
//# sourceMappingURL=featureUsage.d.ts.map