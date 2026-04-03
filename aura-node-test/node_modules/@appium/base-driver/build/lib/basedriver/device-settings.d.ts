import type { StringRecord, IDeviceSettings, SettingsUpdateListener } from '@appium/types';
/**
 * Maximum size (in bytes) of a given driver's settings object (which is internal to {@linkcode DeviceSettings}).
 */
export declare const MAX_SETTINGS_SIZE: number;
/**
 * @template T - Settings object shape (string-keyed record)
 */
export declare class DeviceSettings<T extends StringRecord = StringRecord> implements IDeviceSettings<T> {
    protected _settings: T;
    protected _onSettingsUpdate: SettingsUpdateListener<T>;
    /**
     * Creates a _shallow copy_ of the `defaultSettings` parameter!
     *
     * @param defaultSettings - Initial settings (shallow-copied).
     * @param onSettingsUpdate - Called when a setting is changed; receives (prop, newValue, curValue).
     */
    constructor(defaultSettings?: T, onSettingsUpdate?: SettingsUpdateListener<T>);
    /**
     * Calls updateSettings from implementing driver every time a setting is changed.
     *
     * @param newSettings - New settings to merge (must be plain object; total size remains bounded).
     */
    update(newSettings: T): Promise<void>;
    getSettings(): T;
}
//# sourceMappingURL=device-settings.d.ts.map