export declare const BIDI_COMMANDS: {
    readonly session: {
        readonly subscribe: {
            readonly command: "bidiSubscribe";
            readonly params: {
                readonly required: readonly ["events"];
                readonly optional: readonly ["contexts"];
            };
        };
        readonly unsubscribe: {
            readonly command: "bidiUnsubscribe";
            readonly params: {
                readonly required: readonly ["events"];
                readonly optional: readonly ["contexts"];
            };
        };
        readonly status: {
            readonly command: "bidiStatus";
            readonly params: {};
        };
    };
    readonly browsingContext: {
        readonly navigate: {
            readonly command: "bidiNavigate";
            readonly params: {
                readonly required: readonly ["context", "url"];
                readonly optional: readonly ["wait"];
            };
        };
    };
};
//# sourceMappingURL=bidi-commands.d.ts.map