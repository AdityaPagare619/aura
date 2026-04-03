import type { NextFunction, Request, RequestHandler, Response } from 'express';
import type { IncomingMessage } from 'node:http';
import type { Duplex } from 'node:stream';
export { handleIdempotency } from './idempotency';
import type { StringRecord, WSServer } from '@appium/types';
/**
 * Basic CORS middleware.
 * Sets permissive CORS headers and responds immediately to `OPTIONS` requests with 200.
 */
export declare function allowCrossDomain(req: Request, res: Response, next: NextFunction): void;
/**
 * CORS middleware for async execute response endpoints only.
 * Leaves other routes untouched but applies {@link allowCrossDomain} to async response URLs.
 *
 * @param basePath - Server base path (e.g. `/wd/hub` or `/`)
 * @returns Express request handler
 */
export declare function allowCrossDomainAsyncExecute(basePath: string): RequestHandler;
/**
 * Populates the logger's async context with request and session metadata.
 * Derives `requestId`, optional session id/signature, and `isSensitive` flag from headers/URL.
 */
export declare function handleLogContext(req: Request, _res: Response, next: NextFunction): void;
/**
 * Ensures requests default to JSON content-type when none is provided.
 */
export declare function defaultToJSONContentType(req: Request, _res: Response, next: NextFunction): void;
/**
 * Attempts to handle a WebSocket upgrade by matching the request path against registered handlers.
 *
 * @param req - Incoming HTTP request
 * @param socket - Network socket
 * @param head - First packet of the upgraded stream
 * @param webSocketsMapping - Path-to-WebSocket-server mapping
 * @returns `true` if the upgrade was handled; `false` otherwise
 */
export declare function tryHandleWebSocketUpgrade(req: IncomingMessage, socket: Duplex, head: Buffer, webSocketsMapping: StringRecord<WSServer>): boolean;
/**
 * Express middleware wrapper around {@link tryHandleWebSocketUpgrade}.
 * Delegates WebSocket upgrades to the mapping and falls through to `next()` otherwise.
 *
 * @param webSocketsMapping - Path-to-WebSocket-server mapping
 * @returns Express request handler
 */
export declare function handleUpgrade(webSocketsMapping: StringRecord<WSServer>): RequestHandler;
/**
 * Final error-handling middleware.
 * Logs uncaught errors and returns a W3C-formatted error response unless headers were already sent.
 */
export declare function catchAllHandler(err: Error, _req: Request, res: Response, next: NextFunction): void;
/**
 * 404 handler for unmatched routes.
 * Logs a debug message and responds with `UnknownCommandError` in W3C format.
 */
export declare function catch404Handler(req: Request, res: Response): void;
//# sourceMappingURL=middleware.d.ts.map