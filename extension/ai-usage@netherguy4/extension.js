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
        // Значка нет намеренно: он не нёс информации, а место в панели занимал.
        this._label = new St.Label({
            text: 'AI',
            y_align: Clutter.ActorAlign.CENTER,
            style_class: 'ai-usage-panel-label',
        });
        this._indicator.add_child(this._label);
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
    }

    _render() {
        const accounts = Array.isArray(this._state?.accounts) ? this._state.accounts : [];
        this._indicator.menu.removeAll();
        this._addItem('AI Usage', true);

        if (accounts.length === 0) {
            this._label.text = 'AI';
            this._addItem('Аккаунты не настроены');
            this._addItem('Запусти: ai-usage setup');
            return;
        }

        const summary = [];

        accounts.forEach((account, index) => {
            if (index > 0)
                this._indicator.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

            const subtitle = [account.plan, account.email].filter(Boolean).join(' · ');
            this._addItem(`${providerIcon(account.provider)} ${account.name}${subtitle ? ` — ${subtitle}` : ''}`, true);

            const windows = Array.isArray(account.windows) ? account.windows : [];
            const hasBalances = Array.isArray(account.balances) && account.balances.length > 0;
            const stale = account.status === 'stale';

            for (const window of windows)
                this._addItem(formatWindow(window));

            const headline = panelWindow(windows);
            if (hasBalances) {
                for (const balance of account.balances)
                    this._addItem(`Баланс: ${formatMoney(balance.total, balance.currency)}`);
            }

            // Возраст показываем всегда, когда данные есть: без него нельзя
            // отличить свежий ноль от давно не обновлявшегося.
            if (windows.length || hasBalances) {
                const age = formatAge(account.updated_at);
                if (age)
                    this._addItem(`Обновлено: ${age}`, false, stale ? 'ai-usage-stale' : null);
            }

            if (!windows.length && !hasBalances) {
                // Данных нет вовсе — показываем причину вместо пустоты.
                this._addItem(account.error || statusText(account.status), false, 'ai-usage-error');
            } else if (account.error) {
                // Данные есть, но устарели или последнее обновление не удалось.
                this._addItem(`⚠ ${account.error}`, false, stale ? 'ai-usage-stale' : 'ai-usage-error');
            }

            summary.push({
                icon: providerIcon(account.provider),
                remaining: headline === null ? null : effectiveRemaining(headline),
                // У провайдера может не быть лимитов вовсе — только баланс.
                // Тогда в панель идут деньги, иначе аккаунт выглядел бы сломанным.
                balance: headline === null && hasBalances
                    ? formatMoney(account.balances[0].total, account.balances[0].currency)
                    : null,
                stale,
            });
        });

        this._label.text = panelLabel(summary);
    }

    _addItem(text, header = false, styleClass = null) {
        const item = new PopupMenu.PopupMenuItem(text, {reactive: false});
        if (header)
            item.label.add_style_class_name('ai-usage-header');
        if (styleClass)
            item.label.add_style_class_name(styleClass);
        this._indicator.menu.addMenuItem(item);
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

const FIVE_HOUR_MINS = 300;
const WEEKLY_MINS = 10080;

// Какое окно выносить в панель.
//
// Не минимум по всем окнам: у Claude отдельный лимит модели может быть
// исчерпан, и панель показывала бы 0%, хотя работать ещё можно. Нужен тот
// лимит, который упирается первым в обычной работе, — пятичасовой; если у
// тарифа его нет (Codex plus), то недельный.
//
// windows отсортированы по длительности, затем по ключу, поэтому первое
// совпадение по длительности — самое общее окно, а не привязанное к модели.
function panelWindow(windows) {
    if (!windows.length)
        return null;
    return windows.find(w => w.duration_mins === FIVE_HOUR_MINS)
        ?? windows.find(w => w.duration_mins === WEEKLY_MINS)
        ?? windows[0];
}

function effectiveRemaining(window) {
    if (window.resets_at && Number(window.resets_at) <= Math.floor(Date.now() / 1000))
        return 100;
    return Math.max(0, Math.min(100, Number(window.remaining_percent) || 0));
}

function formatWindow(window) {
    const remaining = effectiveRemaining(window);
    const reset = formatReset(window.resets_at);
    const label = window.label || window.key || 'Лимит';
    return `${label}: ${Math.round(remaining)}% осталось${reset ? ` · ${reset}` : ''}`;
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

// Метка панели: значок провайдера и его худший остаток, по одному на аккаунт.
// Общий минимум по всем аккаунтам был неоднозначен — по числу нельзя было
// понять, чей лимит заканчивается.
//
// '~' помечает устаревшие данные, '!' — аккаунт без данных вовсе.
function panelLabel(summary) {
    if (!summary.length)
        return 'AI';

    const parts = summary.map(entry => {
        const mark = entry.stale ? '~' : '';
        if (entry.remaining !== null)
            return `${entry.icon} ${Math.round(entry.remaining)}%${mark}`;
        if (entry.balance)
            return `${entry.icon} ${entry.balance}${mark}`;
        return `${entry.icon} !`;
    });
    return parts.join(' ');
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
