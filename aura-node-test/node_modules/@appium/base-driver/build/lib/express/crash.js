"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.produceError = produceError;
exports.produceCrash = produceCrash;
const protocol_1 = require("../protocol");
/**
 * Route handler that throws {@link errors.UnknownCommandError} for testing.
 */
function produceError() {
    throw new protocol_1.errors.UnknownCommandError('Produced generic error for testing');
}
/**
 * Route handler that throws a generic Error for testing (e.g. crash scenarios).
 */
function produceCrash() {
    throw new Error('We just tried to crash Appium!');
}
//# sourceMappingURL=crash.js.map