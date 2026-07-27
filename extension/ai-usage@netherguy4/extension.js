import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const REFRESH_SECONDS = 60;

export default class AiUsageExtension extends Extension {
    enable() {
        this._state = null;
        this._reloadSource = 0;

        this._indicator = new PanelMenu.Button(0.0, this.metadata.name, false);
        const box = new St.BoxLayout({style_class: 'panel-status-menu-box'});
        this._icon = new St.Icon({
            icon_name: 'utilities-system-monitor-symbolic',
            style_class: 'system-status-icon',
        });
        this._label = new St.Label({
            text: 'AI',
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'ai-usage-panel-label',
        });
        box.add_child(this._icon);
        box.add_child(this._label);
        this._indicator.add_child(box);
        Main.panel.addToStatusArea(this.uuid, this._indicator, 1, 'right');

        const runtimeDir = GLib.get_user_runtime_dir();
        this._stateFile = Gio.File.new_for_path(
            GLib.build_filenamev([runtimeDir, 'ai-usage', 'state.json'])
        );
        try {
            this._monitor = this._stateFile.monitor_file(Gio.FileMonitorFlags.NONE, null);
            this._monitorChangedId = this._monitor.connect('changed', () => this._queueReload());
        } catch (error) {
            console.warn(`[AI Usage] file monitor unavailable: ${error}`);
        }

        this._timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, REFRESH_SECONDS, () => {
            this._loadState();
            return GLib.SOURCE_CONTINUE;
        });
        this._loadState();
    }

    disable() {
        if (this._reloadSource) {
            GLib.source_remove(this._reloadSource);
            this._reloadSource = 0;
        }
        if (this._timer) {
            GLib.source_remove(this._timer);
            this._timer = 0;
        }
        if (this._monitor && this._monitorChangedId) {
            this._monitor.disconnect(this._monitorChangedId);
        }
        this._monitor?.cancel();
        this._monitor = null;
        this._monitorChangedId = 0;
        this._indicator?.destroy();
        this._indicator = null;
        this._stateFile = null;
        this._state = null;
    }

    _queueReload() {
        if (this._reloadSource)
            return;
        this._reloadSource = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 200, () => {
            this._reloadSource = 0;
            this._loadState();
            return GLib.SOURCE_REMOVE;
        });
    }

    _loadState() {
        try {
            const [, contents] = this._stateFile.load_contents(null);
            this._state = JSON.parse(new TextDecoder().decode(contents));
            this._render();
        } catch (error) {
            this._state = null;
            this._label.text = 'AI …';
            this._renderEmpty(error);
        }
    }

    _renderEmpty(error) {
        this._indicator.menu.removeAll();
        this._addItem('AI Usage', true);
        this._addItem('Ожидание данных от ai-usage.service');
        const message = String(error?.message ?? error ?? 'state.json отсутствует');
        this._addItem(message.length > 80 ? `${message.slice(0, 77)}…` : message);
        this._addRefreshItem();
    }

    _render() {
        const accounts = Array.isArray(this._state?.accounts) ? this._state.accounts : [];
        this._indicator.menu.removeAll();
        this._addItem('AI Usage', true);

        if (accounts.length === 0) {
            this._label.text = 'AI';
            this._addItem('Аккаунты не настроены');
            this._addItem('Запусти: ai-usage setup');
            this._addRefreshItem();
            return;
        }

        let minimumRemaining = null;
        let hasError = false;
        let anyStale = false;

        accounts.forEach((account, index) => {
            if (index > 0)
                this._indicator.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

            const subtitle = [account.plan, account.email].filter(Boolean).join(' · ');
            this._addItem(`${providerIcon(account.provider)} ${account.name}${subtitle ? ` — ${subtitle}` : ''}`, true);

            const hasQuota = Boolean(account.five_hour || account.weekly);
            const hasBalances = Array.isArray(account.balances) && account.balances.length > 0;
            const stale = account.status === 'stale';

            if (account.five_hour) {
                this._addItem(formatWindow('5 часов', account.five_hour));
                if (!stale)
                    minimumRemaining = minValue(minimumRemaining, effectiveRemaining(account.five_hour));
            }
            if (account.weekly) {
                this._addItem(formatWindow('Неделя', account.weekly));
                if (!stale)
                    minimumRemaining = minValue(minimumRemaining, effectiveRemaining(account.weekly));
            }
            if (hasBalances) {
                for (const balance of account.balances)
                    this._addItem(`Баланс: ${formatMoney(balance.total, balance.currency)}`);
            }

            // Возраст показываем всегда, когда данные есть: без него нельзя
            // отличить свежий ноль от давно не обновлявшегося.
            if (hasQuota || hasBalances) {
                const age = formatAge(account.updated_at);
                if (age)
                    this._addItem(`Обновлено: ${age}`, false, stale ? 'ai-usage-stale' : null);
            }

            if (!hasQuota && !hasBalances) {
                // Данных нет вовсе — показываем причину вместо пустоты.
                this._addItem(account.error || statusText(account.status), false, 'ai-usage-error');
            } else if (account.error) {
                // Данные есть, но устарели или последнее обновление не удалось.
                this._addItem(`⚠ ${account.error}`, false, stale ? 'ai-usage-stale' : 'ai-usage-error');
            }

            if (stale)
                anyStale = true;
            if (account.status === 'error' || account.status === 'unauthenticated')
                hasError = true;
        });

        this._label.text = panelLabel(minimumRemaining, hasError, anyStale);
        this._addRefreshItem();
    }

    _addItem(text, header = false, styleClass = null) {
        const item = new PopupMenu.PopupMenuItem(text, {reactive: false});
        if (header)
            item.label.add_style_class_name('ai-usage-header');
        if (styleClass)
            item.label.add_style_class_name(styleClass);
        this._indicator.menu.addMenuItem(item);
    }

    _addRefreshItem() {
        this._indicator.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        const refresh = new PopupMenu.PopupMenuItem('Обновить сейчас');
        refresh.connect('activate', () => {
            try {
                Gio.Subprocess.new(
                    ['systemctl', '--user', 'restart', 'ai-usage.service'],
                    Gio.SubprocessFlags.NONE
                );
            } catch (error) {
                console.error(`[AI Usage] refresh failed: ${error}`);
            }
        });
        this._indicator.menu.addMenuItem(refresh);
    }
}

function providerIcon(provider) {
    switch (provider) {
    case 'claude': return '◈';
    case 'codex': return '⌘';
    case 'deepseek': return '◆';
    default: return '•';
    }
}

function minValue(current, value) {
    const numeric = Number(value);
    if (!Number.isFinite(numeric))
        return current;
    return current === null ? numeric : Math.min(current, numeric);
}

function effectiveRemaining(window) {
    if (window.resets_at && Number(window.resets_at) <= Math.floor(Date.now() / 1000))
        return 100;
    return Math.max(0, Math.min(100, Number(window.remaining_percent) || 0));
}

function formatWindow(title, window) {
    const remaining = effectiveRemaining(window);
    const reset = formatReset(window.resets_at);
    return `${title}: ${Math.round(remaining)}% осталось${reset ? ` · ${reset}` : ''}`;
}

function formatReset(timestamp) {
    if (!timestamp)
        return '';
    const seconds = Math.max(0, Number(timestamp) - Math.floor(Date.now() / 1000));
    if (seconds <= 0)
        return 'сброс сейчас';
    const days = Math.floor(seconds / 86400);
    const hours = Math.floor((seconds % 86400) / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    if (days > 0)
        return `сброс через ${days}д ${hours}ч`;
    if (hours > 0)
        return `сброс через ${hours}ч ${minutes}м`;
    return `сброс через ${Math.max(1, minutes)}м`;
}

function formatMoney(value, currency) {
    const number = Number.parseFloat(value);
    if (!Number.isFinite(number))
        return `${value} ${currency}`;
    try {
        return new Intl.NumberFormat('ru-RU', {
            style: 'currency',
            currency,
            minimumFractionDigits: 2,
            maximumFractionDigits: 4,
        }).format(number);
    } catch {
        return `${value} ${currency}`;
    }
}

function statusText(status) {
    switch (status) {
    case 'waiting': return 'Ожидание первого запроса';
    case 'stale': return 'Данные устарели';
    case 'unauthenticated': return 'Требуется вход';
    case 'depleted': return 'Баланс исчерпан';
    case 'error': return 'Провайдер недоступен';
    default: return 'Нет данных';
    }
}

// Метка панели. Устаревшие данные помечаются '!', чтобы процент не выглядел
// свежим; провайдер без данных вовсе — тоже восклицательным знаком.
function panelLabel(minimumRemaining, hasError, anyStale) {
    if (minimumRemaining !== null) {
        const percent = `AI ${Math.round(minimumRemaining)}%`;
        return hasError || anyStale ? `${percent} !` : percent;
    }
    if (anyStale)
        return 'AI ~';
    return hasError ? 'AI !' : 'AI';
}

function formatAge(updatedAt) {
    const timestamp = Number(updatedAt);
    if (!Number.isFinite(timestamp) || timestamp <= 0)
        return '';
    const seconds = Math.floor(Date.now() / 1000) - timestamp;
    // Часы пользователя могут отставать от времени записи state.json.
    if (seconds < 0)
        return 'только что';
    if (seconds < 90)
        return 'только что';
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60)
        return `${minutes} мин назад`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24)
        return `${hours} ч назад`;
    return `${Math.floor(hours / 24)} д назад`;
}
