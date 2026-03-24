import { expect } from '@wdio/globals';
import { SnapshotClient } from '@vitest/snapshot';
import { NodeSnapshotEnvironment } from '@vitest/snapshot/environment';
let service;
class WebdriverIOSnapshotEnvironment extends NodeSnapshotEnvironment {
    #resolveSnapshotPath;
    constructor(resolveSnapshotPath) {
        super({});
        this.#resolveSnapshotPath = resolveSnapshotPath;
    }
    async resolvePath(filepath) {
        if (this.#resolveSnapshotPath) {
            return this.#resolveSnapshotPath(filepath, '.snap');
        }
        return super.resolvePath(filepath);
    }
}
export class SnapshotService {
    #currentFilePath;
    #currentTestName;
    #options;
    #snapshotResults = [];
    #snapshotClient = new SnapshotClient({
        isEqual: this.#isEqual.bind(this),
    });
    constructor(options) {
        const updateSnapshot = (Boolean(process.env.CI) && !options?.updateState)
            ? 'none'
            : options?.updateState
                ? options.updateState
                : 'new';
        const snapshotFormatConfig = options?.snapshotFormat ? {
            printBasicPrototype: false,
            escapeString: false,
            ...options.snapshotFormat
        } : undefined;
        this.#options = {
            updateSnapshot,
            snapshotEnvironment: new WebdriverIOSnapshotEnvironment(options?.resolveSnapshotPath),
            ...(snapshotFormatConfig && { snapshotFormat: snapshotFormatConfig })
        };
    }
    get currentFilePath() {
        return this.#currentFilePath;
    }
    get currentTestName() {
        return this.#currentTestName;
    }
    get client() {
        return this.#snapshotClient;
    }
    get results() {
        return this.#snapshotResults;
    }
    async beforeTest(test) {
        this.#currentFilePath = test.file;
        this.#currentTestName = `${test.parent} > ${test.title}`;
        await this.#snapshotClient.setup(test.file, this.#options);
    }
    async beforeStep(step, scenario) {
        const file = scenario.uri;
        const testName = `${scenario.name} > ${step.text}`;
        this.#currentFilePath = file;
        this.#currentTestName = testName;
        await this.#snapshotClient.setup(file, this.#options);
    }
    async after() {
        if (!this.#currentFilePath) {
            return;
        }
        const result = await this.#snapshotClient.finish(this.#currentFilePath);
        if (!result) {
            return;
        }
        this.#snapshotResults.push(result);
    }
    #isEqual(received, expected) {
        try {
            expect(received).toBe(expected);
            return true;
        }
        catch {
            return false;
        }
    }
    static initiate(options) {
        if (!service) {
            service = new SnapshotService(options);
        }
        return service;
    }
}
