import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import St from 'gi://St';

import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const REFRESH_SECONDS = 60;
const FIVE_HOUR_MINS = 300;
const WEEKLY_MINS = 10080;

export default class AiUsageExtension extends Extension {
    enable() {
        this._state = null;
        this._reloadSource = 0;
        this._iconDir = this.dir.get_child('icons');

        this._indicator = new PanelMenu.Button(0.0, this.metadata.name, false);
        this._panelBox = new St.BoxLayout({style_class: 'ai-usage-panel-box'});
        this._indicator.add_child(this._panelBox);
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
        this._panelBox = null;
        this._stateFile = null;
        this._iconDir = null;
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
            this._renderPanel([{provider: null, text: '…'}]);
            this._renderEmpty(error);
        }
    }

    /// Иконка провайдера из каталога расширения. Имя оканчивается на
    /// `-symbolic.svg`, поэтому GNOME перекрашивает её под тему панели.
    _providerIcon(provider) {
        if (!provider || !this._iconDir)
            return null;
        const file = this._iconDir.get_child(`ai-usage-${provider}-symbolic.svg`);
        return file.query_exists(null) ? new Gio.FileIcon({file}) : null;
    }

    _renderEmpty(error) {
        this._indicator.menu.removeAll();
        this._addItem('AI Usage', {header: true});
        this._addItem('Ожидание данных от ai-usage.service');
        const message = String(error?.message ?? error ?? 'state.json отсутствует');
        this._addItem(message.length > 80 ? `${message.slice(0, 77)}…` : message);
    }

    _render() {
        const accounts = Array.isArray(this._state?.accounts) ? this._state.accounts : [];
        this._indicator.menu.removeAll();
        this._addItem('AI Usage', {header: true});

        if (accounts.length === 0) {
            this._renderPanel([]);
            this._addItem('Аккаунты не настроены');
            this._addItem('Запусти: ai-usage setup');
            return;
        }

        const summary = [];

        accounts.forEach((account, index) => {
            if (index > 0)
                this._indicator.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

            const subtitle = [account.plan, account.email].filter(Boolean).join(' · ');
            this._addItem(`${account.name}${subtitle ? ` — ${subtitle}` : ''}`, {
                header: true,
                gicon: this._providerIcon(account.provider),
            });

            const windows = Array.isArray(account.windows) ? account.windows : [];
            const balances = Array.isArray(account.balances) ? account.balances : [];
            const stale = account.status === 'stale';

            for (const window of windows)
                this._addItem(formatWindow(window));
            for (const balance of balances)
                this._addItem(`Баланс: ${formatMoney(balance.total, balance.currency)}`);

            if (!windows.length && !balances.length) {
                // Данных нет вовсе — показываем причину вместо пустоты.
                this._addItem(account.error || statusText(account.status), {style: 'ai-usage-error'});
            } else if (account.error) {
                // Данные есть, но устарели или последнее обновление не удалось.
                // Возраст живёт в тексте ошибки: отдельной строки «Обновлено»
                // нет, пока всё в порядке она была лишним шумом.
                this._addItem(`⚠ ${account.error}`, {style: stale ? 'ai-usage-stale' : 'ai-usage-error'});
            }

            summary.push({
                provider: account.provider,
                text: panelText(windows, balances, stale),
            });
        });

        this._renderPanel(summary);
    }

    _renderPanel(summary) {
        if (!this._panelBox)
            return;
        this._panelBox.destroy_all_children();

        if (!summary.length) {
            this._panelBox.add_child(new St.Label({
                text: 'AI',
                y_align: Clutter.ActorAlign.CENTER,
                style_class: 'ai-usage-panel-label',
            }));
            return;
        }

        for (const entry of summary) {
            // Значок и число живут в собственном контейнере: так пара держится
            // вместе, а расстояние между аккаунтами задаётся отдельно.
            const item = new St.BoxLayout({style_class: 'ai-usage-panel-item'});
            const gicon = this._providerIcon(entry.provider);
            if (gicon) {
                item.add_child(new St.Icon({
                    gicon,
                    y_align: Clutter.ActorAlign.CENTER,
                    style_class: 'ai-usage-panel-icon',
                }));
            }
            item.add_child(new St.Label({
                text: entry.text,
                y_align: Clutter.ActorAlign.CENTER,
                style_class: 'ai-usage-panel-label',
            }));
            this._panelBox.add_child(item);
        }
    }

    _addItem(text, {header = false, style = null, gicon = null} = {}) {
        // Собираем строку сами, а не через PopupImageMenuItem: порядок детей
        // у базового пункта меняется между версиями GNOME.
        const item = new PopupMenu.PopupBaseMenuItem({reactive: false});
        const box = new St.BoxLayout({style_class: 'ai-usage-menu-row'});
        if (gicon)
            box.add_child(new St.Icon({gicon, style_class: 'popup-menu-icon ai-usage-menu-icon'}));

        const label = new St.Label({text, y_align: Clutter.ActorAlign.CENTER});
        if (header)
            label.add_style_class_name('ai-usage-header');
        if (style)
            label.add_style_class_name(style);
        box.add_child(label);

        item.add_child(box);
        this._indicator.menu.addMenuItem(item);
    }
}

// Какое окно выносить в панель.
//
// Правило блокировки идёт первым: если исчерпан общий лимит, запас в коротком
// окне уже ничего не значит — работать нельзя, и панель обязана показать 0%.
//
// Лимиты, привязанные к модели, из этого правила исключены: исчерпанный лимит
// одной модели не блокирует работу, достаточно переключиться. Именно поэтому
// панель не берёт минимум по всем окнам подряд.
//
// В остальном показывается пятичасовое окно, а при его отсутствии (Codex plus)
// — недельное. windows отсортированы по длительности и ключу, поэтому первое
// совпадение по длительности — самое общее окно.
function panelWindow(windows) {
    if (!windows.length)
        return null;

    const general = windows.filter(w => !w.scope);
    const pool = general.length ? general : windows;

    const blocked = pool.find(w => effectiveRemaining(w) <= 0);
    if (blocked)
        return blocked;

    return pool.find(w => w.duration_mins === FIVE_HOUR_MINS)
        ?? pool.find(w => w.duration_mins === WEEKLY_MINS)
        ?? pool[0];
}

// Текст рядом со значком провайдера в панели.
function panelText(windows, balances, stale) {
    const mark = stale ? '~' : '';
    const headline = panelWindow(windows);
    if (headline)
        return `${Math.round(effectiveRemaining(headline))}%${mark}`;
    // У провайдера может не быть лимитов вовсе — только баланс.
    if (balances.length)
        return `${formatMoney(balances[0].total, balances[0].currency)}${mark}`;
    return '!';
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
