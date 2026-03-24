import type { BrowserstackConfig, UserConfig } from '../types.js';
import type { Options } from '@wdio/types';
declare class Percy {
    #private;
    isProcessRunning: boolean;
    percyCaptureMode?: string;
    buildId: number | null;
    percyAutoEnabled: boolean;
    percy: boolean;
    constructor(options: BrowserstackConfig & Options.Testrunner, config: Options.Testrunner, bsConfig: UserConfig);
    healthcheck(): Promise<boolean | undefined>;
    start(): Promise<boolean>;
    stop(): Promise<unknown>;
    isRunning(): boolean;
    fetchPercyToken(): Promise<any>;
    createPercyConfig(): Promise<unknown>;
}
export default Percy;
//# sourceMappingURL=Percy.d.ts.map