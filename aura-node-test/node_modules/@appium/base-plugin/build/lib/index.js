"use strict";
var __importDefault = (this && this.__importDefault) || function (mod) {
    return (mod && mod.__esModule) ? mod : { "default": mod };
};
Object.defineProperty(exports, "__esModule", { value: true });
exports.default = exports.BasePlugin = void 0;
var plugin_1 = require("./plugin");
Object.defineProperty(exports, "BasePlugin", { enumerable: true, get: function () { return plugin_1.BasePlugin; } });
Object.defineProperty(exports, "default", { enumerable: true, get: function () { return __importDefault(plugin_1).default; } });
// Handle smoke test flag
if (require.main === module && process.argv[2] === '--smoke-test') {
    process.exit(0);
}
//# sourceMappingURL=index.js.map