export interface AISdkConfig {
    domain: string;
    platform: "desktop"|"mobile"|"app";
    connector: "direct"|"extension";
    client: string;
}

export interface CompatibilityCheckDetails {
    browserName: string;
    version: string;
}

export class AISDK {
    static configure(config: AISdkConfig): void;
    static checkExtensionCompatibility(details: CompatibilityCheckDetails): boolean;
}

export namespace BrowserstackHealing {
    type InitSuccessResponse = {
        isAuthenticated: true;
        userId: number;
        groupId: number;
        sessionToken: string;
        isGroupAIEnabled: boolean;
        isHealingEnabled: boolean;
        defaultLogDataEnabled: boolean;
    };

    type InitErrorResponse = {
        isAuthenticated: false;
        message?: string;
        status?: number;
    };

    function init(
        accessToken: string,
        userName: string,
        url: string,
        sdkVersion: string
    ): Promise<InitSuccessResponse | InitErrorResponse>;

    function setToken(
        sessionId: string,
        accessToken: string,
        url: string
    ): Promise<void>;

    function logData(
        locatorType: string,
        locatorValue: string,
        projectName?: string,
        testName?: string,
        groupId?: number,
        sessionId?: string,
        listOfCommands?: string,
        tcgEndpoint?: string,
        sessionToken?: string,
        referenceId?: string | null,
        rootId?: string | null,
        isGetShadowRoot?: boolean
    ): Promise<string>;

    function healFailure(
        locatorType: string,
        locatorValue: string,
        projectName?: string,
        testName?: string,
        userId?: number,
        groupId?: number,
        sessionId?: string,
        listOfCommands?: string,
        logs?: boolean,
        groupAIEnabled?: boolean,
        tcgEndpoint?: string,
        sessionToken?: string
    ): Promise<string>;

    function pollResult(
        url: string,
        sessionId: string,
        accessToken: string
    ): Promise<{ selector: string; value: string } | null>;

    function initializeCapabilities(
        capabilities: object
    ): object;

    function getFirefoxAddonPath(): string;
}

export interface VariableInput {
    name: string;
    id: string;
}

export interface InputValue {
    type: 'variable' | 'function' | 'ai' | 'text';
    value: string;
    defaultValue: string | undefined;
    args: InputArg[] | undefined;
}

export interface InputArg {
    name: string;
    value: any;
}

export interface FunctionDef {
    name: string;
    description: string;
    args: { name: string; type: 'string' | 'boolean' | 'number' }[];
    definition: string;
}

export namespace NLToSteps {
    interface ModuleDefinition {
        name: string;
        description: string;
        start: string;
    }

    interface NLToStepsFrameworkImpl {
        executeScript: (script: (...data: any) => any, args: any[]) => any | Promise<any>;
        executeAsyncScript?: (script: (...data: any) => any, args: any[]) => Promise<any>;
        executeAllFrames?: (script: string) => Promise<void>;
        getBrowser: () => string | Promise<string>;
        getOS?: () => Promise<string>;
    }

    interface NLToStepsRequest {
        id: string;
        objective: string;
        supportedActions?: string[];
        variables?: VariableInput[];
        functions?: FunctionDef[];
        modules?: ModuleDefinition[];
        useAIGenerator?: boolean;
        waitForCustomActions?: boolean;
        waitForNetworkCalls?: boolean;
        waitAfterActions?: boolean;
        waitCallback: (waitingAction: NLToStepsWaitAction) => Promise<any> | any;
        authMethod: () => Promise<string> | string;
        frameworkImplementation: NLToStepsFrameworkImpl;
    }

    interface NLToStepsInputRequest {
        input: string;
        parsedInput: InputValue[];
    }

    interface NLToStepsWaitStep {
        action: Object;
        stepId: string;
    }

    interface NLToStepsWaitAction {
        type: 'INPUT' | 'MODULE' | 'STEP' | 'TCG' | 'WAIT_BEFORE_TCG_CALL' | 'WAIT_AFTER_TCG_CALL' | 'BEFORE_STEP';
        id: string;
        request: NLToStepsInputRequest | NLToStepsAction;
    }

    interface NLToStepsStatus {
        state: 'RUNNING' | 'FAILED' | 'SUCCESS' | 'WAITING';
        request: NLToStepsRequest;
        failReason: string;
        errorName: string;
        currentWaitAction: NLToStepsWaitAction;
    }

    interface NLToStepsResponse {
        id: string;
        state: 'RUNNING' | 'FAILED' | 'SUCCESS' | 'WAITING';
        failReason: string;
        errorName: string;
        performedActions: NLToStepsAction[];
    }

    export interface NLToStepsAction {
        action_type: string;
        element: ActionElement;
        suggestionMeta: SuggestionMeta;
    }

    export interface ActionElement {
        attributes: ActionElementAttributes;
        coordinates: ActionElementCoordinates;
        input_value: string;
        selectorsObj: ActionElementSelectors;
        tag_name: string;
        xpath: string;
        extracted_value: string;
    }

    export interface ActionElementAttributes {
        heading: string;
        id: string;
        name: string;
    }

    export interface ActionElementCoordinates {
        x: number;
        y: number;
    }

    export interface ActionElementSelectors {
        selectTagOptionAttributes: SelectTagOptionAttribute[];
    }

    export interface SelectTagOptionAttribute {
        innerText: string;
    }

    export interface SuggestionMeta {
        identifier: string;
        rawAction: RawAction;
        suggestionId: string;
    }

    export interface RawAction {
        action: string;
        id: number;
        input: string;
        thought: string;
        value: string;
    }

    interface ExecutorActionHandler {
        callTCG: (endpoint: string, method: 'POST' | 'GET' | 'PUT' | 'DELETE' | 'PATCH', request: any, timeout: number) => Promise<any>;
        doAfterStepAction: (stepData: NLToStepsAction, timeout: number) => Promise<boolean>;
        doBeforeStepAction: (stepData: NLToStepsAction, timeout: number) => Promise<boolean>;
        waitForInput: (input: NLToStepsInputRequest, timeout: number) => Promise<string>;
    }

    function start(request: NLToStepsRequest): Promise<NLToStepsResponse>;

    function cancelNL2StepsExecution(requestId: string, reason: string): Promise<NLToStepsResponse>;
}
