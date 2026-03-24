import type { Request, Response, NextFunction } from 'express';
/**
 * Middleware that caches and replays responses for idempotent requests using the
 * `x-idempotency-key` header. Only POST and PATCH are cached.
 */
export declare function handleIdempotency(req: Request, res: Response, next: NextFunction): Promise<void>;
//# sourceMappingURL=idempotency.d.ts.map