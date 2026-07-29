# Иконки провайдеров

| Файл | Что это | Источник |
|---|---|---|
| `ai-usage-claude-symbolic.svg` | знак Claude | [lobe-icons](https://github.com/lobehub/lobe-icons), MIT |
| `ai-usage-codex-symbolic.svg` | знак OpenAI | [lobe-icons](https://github.com/lobehub/lobe-icons), MIT |
| `ai-usage-deepseek-symbolic.svg` | знак DeepSeek | [simple-icons](https://github.com/simple-icons/simple-icons), CC0 |
| `ai-usage-antigravity-symbolic.svg` | нейтральный орбитальный знак для Antigravity | собственная геометрия проекта, GPL-3.0-or-later |

Файлы приведены к одному виду: `viewBox`, `fill="currentColor"` и суффикс
`-symbolic.svg`, чтобы GNOME перекрашивал их под тему панели. Геометрия внешних
логотипов не менялась; Antigravity намеренно использует нейтральный знак, а не
товарный знак Google.

**Товарные знаки.** Лицензии выше относятся к SVG-файлам, но не к самим
логотипам: они остаются товарными знаками Anthropic, OpenAI и DeepSeek
соответственно. Здесь они используются только для того, чтобы обозначить, к
какому сервису относится строка — это номинативное использование, а не знак
принадлежности или одобрения. Правообладатель может попросить убрать их;
`simple-icons` уже удалил логотип OpenAI по такому запросу.

Если это неприемлемо для вашей сборки, замените файлы на нейтральные — код
берёт их по имени `ai-usage-<provider>-symbolic.svg` и не зависит от
содержимого.
