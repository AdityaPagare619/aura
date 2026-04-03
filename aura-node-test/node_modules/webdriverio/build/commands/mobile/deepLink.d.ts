/**
 *
 * Open a deep link URL in the mobile app based on the url and the app's package name (Android) or bundle ID (iOS).
 *
 * <example>
    :deeplink.js
    it('should open a deep link for the WDIO native demo app', async () => {
        // open the Drag tab with a deep link (this the bundleId for the iOS Demo App)
        await browser.deepLink('wdio://drag', 'org.reactjs.native.example.wdiodemoapp');

        // Or open the Drag tab with a deep link (this the package name for the Android Demo App)
        await browser.deepLink('wdio://drag', 'com.wdiodemoapp');

        // Or if you want to have it "cross-platform" you can use it like this
        await browser.deepLink('wdio://drag', browser.isIOS ? 'org.reactjs.native.example.wdiodemoapp' : 'com.wdiodemoapp');
    })
    it('should open a deep link without waiting for the app to launch (Android)', async () => {
        // Android-only: do not wait for the app to launch after opening the deep link
        await browser.deepLink('wdio://drag', 'com.wdiodemoapp', false)
    })
 * </example>
 *
 * @param {string}   link             The deep link URL that should be opened in the mobile app. It should be a valid deep link URL (e.g. `myapp://path`). If it's a universal deep link, which can be used for iOS, use the `browser.url("your-url")`-method.
 * @param {string}   appIdentifier    The value of the `package` (Android) or `bundleId` (iOS) of the app that the deep link should open.
 * @param {boolean}  [waitForLaunch]  Whether to wait for the app to launch after opening the deep link. Default is `true`. <br /><strong>ANDROID-ONLY</strong>
 *
 * @support ["ios","android"]
 */
export declare function deepLink(this: WebdriverIO.Browser, link: string, appIdentifier: string, waitForLaunch?: boolean): Promise<unknown>;
//# sourceMappingURL=deepLink.d.ts.map