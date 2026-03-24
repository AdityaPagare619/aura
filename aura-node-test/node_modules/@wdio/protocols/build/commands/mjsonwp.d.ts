export default interface MjsonwpCommands {
    /**
     * Mjsonwp Protocol Command
     *
     * No description available, please see reference link.
     * @ref https://github.com/appium/appium-base-driver/blob/master/docs/mjsonwp/protocol-methods.md#mobile-json-wire-protocol-endpoints
     * @deprecated In Appium 2.0, this method is marked as deprecated and currently has no available alternatives.
     *
     */
    getPageIndex(): Promise<string>;
    /**
     * Mjsonwp Protocol Command
     *
     * No description available, please see reference link.
     * @ref https://github.com/SeleniumHQ/mobile-spec/blob/master/spec-draft.md#device-modes
     * @deprecated For Appium Android automation, use `driver.execute('mobile: getConnectivity', { ... })` instead
     *
     */
    getNetworkConnection(): Promise<number>;
    /**
     * Mjsonwp Protocol Command
     *
     * No description available, please see reference link.
     * @ref https://github.com/SeleniumHQ/mobile-spec/blob/master/spec-draft.md#device-modes
     * @deprecated For Appium Android automation, use `driver.execute('mobile: setConnectivity', { ... })` instead
     *
     */
    setNetworkConnection(type: number): Promise<void>;
    /**
     * Mjsonwp Protocol Command
     *
     * No description available, please see reference link.
     * @ref https://github.com/SeleniumHQ/mobile-spec/blob/master/spec-draft.md#touch-gestures
     * @deprecated For Appium, use `driver.performActions(...)` instead
     *
     */
    touchPerform(actions: object[]): Promise<void>;
    /**
     * Mjsonwp Protocol Command
     *
     * No description available, please see reference link.
     * @ref https://github.com/SeleniumHQ/mobile-spec/blob/master/spec-draft.md#touch-gestures
     * @deprecated For Appium, use `driver.performActions(...)` instead
     *
     */
    multiTouchPerform(actions: object[], elementId?: object[]): Promise<void>;
    /**
     * Mjsonwp Protocol Command
     *
     * No description available, please see reference link.
     * @ref https://github.com/appium/appium-base-driver/blob/master/docs/mjsonwp/protocol-methods.md#mobile-json-wire-protocol-endpoints
     * @deprecated For Appium, use `driver.executeAsyncScript(...)` instead
     *
     */
    receiveAsyncResponse(status: string, value: string): Promise<void>;
}
//# sourceMappingURL=mjsonwp.d.ts.map