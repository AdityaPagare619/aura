import type { AppiumServer, WSServer } from '@appium/types';
export declare const DEFAULT_WS_PATHNAME_PREFIX = "/ws";
/**
 * Adds a WebSocket handler to this server's mapping.
 * @see AppiumServerExtension.addWebSocketHandler
 */
export declare function addWebSocketHandler(this: AppiumServer, handlerPathname: string, handlerServer: WSServer): Promise<void>;
/**
 * Returns WebSocket handlers for this server, optionally filtered by pathname.
 * @see AppiumServerExtension.getWebSocketHandlers
 */
export declare function getWebSocketHandlers(this: AppiumServer, keysFilter?: string | null): Promise<Record<string, WSServer>>;
/**
 * Removes a WebSocket handler by pathname.
 * @see AppiumServerExtension.removeWebSocketHandler
 */
export declare function removeWebSocketHandler(this: AppiumServer, handlerPathname: string): Promise<boolean>;
/**
 * Removes all WebSocket handlers from this server.
 * @see AppiumServerExtension.removeAllWebSocketHandlers
 */
export declare function removeAllWebSocketHandlers(this: AppiumServer): Promise<boolean>;
//# sourceMappingURL=websocket.d.ts.map