export declare const codes: {
    readonly Success: {
        readonly code: 0;
        readonly summary: "The command executed successfully.";
    };
    readonly NoSuchDriver: {
        readonly code: 6;
        readonly summary: "A session is either terminated or not started";
    };
    readonly NoSuchElement: {
        readonly code: 7;
        readonly summary: "An element could not be located on the page using the given search parameters.";
    };
    readonly NoSuchFrame: {
        readonly code: 8;
        readonly summary: "A request to switch to a frame could not be satisfied because the frame could not be found.";
    };
    readonly UnknownCommand: {
        readonly code: 9;
        readonly summary: "The requested resource could not be found, or a request was received using an HTTP method that is not supported by the mapped resource.";
    };
    readonly StaleElementReference: {
        readonly code: 10;
        readonly summary: "An element command failed because the referenced element is no longer attached to the DOM.";
    };
    readonly ElementNotVisible: {
        readonly code: 11;
        readonly summary: "An element command could not be completed because the element is not visible on the page.";
    };
    readonly InvalidElementState: {
        readonly code: 12;
        readonly summary: "An element command could not be completed because the element is in an invalid state (e.g. attempting to click a disabled element).";
    };
    readonly UnknownError: {
        readonly code: 13;
        readonly summary: "An unknown server-side error occurred while processing the command.";
    };
    readonly ElementIsNotSelectable: {
        readonly code: 15;
        readonly summary: "An attempt was made to select an element that cannot be selected.";
    };
    readonly JavaScriptError: {
        readonly code: 17;
        readonly summary: "An error occurred while executing user supplied JavaScript.";
    };
    readonly XPathLookupError: {
        readonly code: 19;
        readonly summary: "An error occurred while searching for an element by XPath.";
    };
    readonly Timeout: {
        readonly code: 21;
        readonly summary: "An operation did not complete before its timeout expired.";
    };
    readonly NoSuchWindow: {
        readonly code: 23;
        readonly summary: "A request to switch to a different window could not be satisfied because the window could not be found.";
    };
    readonly InvalidCookieDomain: {
        readonly code: 24;
        readonly summary: "An illegal attempt was made to set a cookie under a different domain than the current page.";
    };
    readonly UnableToSetCookie: {
        readonly code: 25;
        readonly summary: "A request to set a cookie's value could not be satisfied.";
    };
    readonly UnexpectedAlertOpen: {
        readonly code: 26;
        readonly summary: "A modal dialog was open, blocking this operation";
    };
    readonly NoAlertOpenError: {
        readonly code: 27;
        readonly summary: "An attempt was made to operate on a modal dialog when one was not open.";
    };
    readonly ScriptTimeout: {
        readonly code: 28;
        readonly summary: "A script did not complete before its timeout expired.";
    };
    readonly InvalidElementCoordinates: {
        readonly code: 29;
        readonly summary: "The coordinates provided to an interactions operation are invalid.";
    };
    readonly IMENotAvailable: {
        readonly code: 30;
        readonly summary: "IME was not available.";
    };
    readonly IMEEngineActivationFailed: {
        readonly code: 31;
        readonly summary: "An IME engine could not be started.";
    };
    readonly InvalidSelector: {
        readonly code: 32;
        readonly summary: "Argument was an invalid selector (e.g. XPath/CSS).";
    };
    readonly SessionNotCreatedException: {
        readonly code: 33;
        readonly summary: "A new session could not be created.";
    };
    readonly MoveTargetOutOfBounds: {
        readonly code: 34;
        readonly summary: "Target provided for a move action is out of bounds.";
    };
    readonly NoSuchContext: {
        readonly code: 35;
        readonly summary: "No such context found.";
    };
};
/**
 * Returns the summary message for a JSONWP status code.
 *
 * @param code - Numeric status code (e.g. 0, 7) or string representation
 * @returns Human-readable summary for the code, or 'An error occurred' if unknown
 */
export declare function getSummaryByCode(code: number | string): string;
//# sourceMappingURL=status.d.ts.map