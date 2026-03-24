/**
 *
 * Get the content of the system clipboard as a base64-encoded string.
 *
 * > **Note:** Falls back to the deprecated Appium 2 protocol endpoint if the driver does not support the `mobile:` execute method.
 *
 * Android only supports the `plaintext` content type.
 *
 * <example>
    :getClipboard.js
    it('should get the clipboard content', async () => {
        const content = await browser.getClipboard()
        const decoded = Buffer.from(content, 'base64').toString('utf8')
    })
 * </example>
 *
 * @param {string} [contentType]  Content type to retrieve (e.g. `'plaintext'`, `'image'`, `'url'`). Android only supports `'plaintext'`.
 *
 * @returns {`Promise<string>`} Base64-encoded clipboard content.
 *
 * @support ["ios","android"]
 */
export declare function getClipboard(this: WebdriverIO.Browser, contentType?: string): Promise<string>;
//# sourceMappingURL=getClipboard.d.ts.map