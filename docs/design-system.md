# MyMCPs design system

Reference for the server-rendered UI. Source of truth: the Figma file
[MyMCP](https://www.figma.com/design/RkOcSJ3wZ4gJOl1RlRLDl2/MyMCP) (pages Foundations, Icons,
Components, Screens).

| File | What it is |
| --- | --- |
| `crates/web/assets/css/app.css` | The whole stylesheet: tokens, components, layout. Plain CSS, no build step. |
| `crates/web/assets/js/app.js` | One dependency-free script driven by `data-` attributes. |
| `crates/web/assets/fonts/` | Inter and JetBrains Mono, variable `woff2`, latin and latin-ext, with their OFL licenses. |
| `crates/web/assets/icons/*.svg` | Lucide icons (ISC, `LICENSE` in the folder). `icons/brands/*.svg`: the brand marks of the old gallery. |
| `crates/web/assets/brand/` | `logo-mark.svg`, `logo-mark-on-frame.svg`, `favicon.svg`, `favicon.png`, `apple-touch-icon.png`, `mymcps-app-icon.png`. |

The CSP is `style-src 'self'`, `font-src 'self'`, `script-src 'self'` with a nonce. So: no
`style=""` attribute, no `<style>`, no inline script, nothing from a CDN. A value that depends on
data goes in a class or a `data-` attribute (see [Dynamic values](#dynamic-values)).

## Rules

- Use a component class. Do not write a one-off class for a screen when a component exists.
- Class names are `.block`, `.block__element`, `.block--variant`. State comes from HTML and ARIA:
  `disabled`, `hidden`, `open`, `aria-current="page"`, `aria-selected`, `aria-pressed`,
  `aria-checked`, `aria-invalid`, `aria-busy`, and the `data-` attributes app.js sets.
- Every value in the CSS is a token. In new CSS use `var(--mm-…)`, never a raw colour or size.
- One primary button per screen. Secondary for supporting actions, tertiary for quiet ones,
  critical for the destructive action of a dialog.
- Dense data is a table or list rows inside a card. A card is a standalone widget.
- Status in a table cell is `.status` (dot and plain label). Everywhere else it is `.badge`.
- Controls are 32px (Medium). They become 44px (Large) by themselves under 768px and inside
  `.control-lg` (auth screens); tables, card headers, banners, list rows, pagination, toolbars,
  menus and toasts stay Medium. Force a size with `.button--lg` / `.button--md`.
- Icon-only buttons always have an `aria-label`.
- Every form works without JavaScript as a plain POST.

## Tokens

Names are the Figma variables' own code syntax. Light and dark values are defined once with
`light-dark()`; the theme follows the system. `<html data-theme="light">` or
`<html data-theme="dark">` forces one. There is no theme toggle in the design.

| Group | Tokens |
| --- | --- |
| Background | `--mm-color-bg-frame`, `-frame-hover`, `-frame-selected`, `-canvas`, `-page`, `-surface`, `-surface-secondary`, `-surface-tertiary`, `-input`, `-fill-secondary`, `-fill-secondary-hover`, `-fill-brand`, `-fill-brand-hover`, `-fill-critical`, `-fill-critical-hover`, `-fill-disabled`, `-brand-subtle`, `-neutral`, `-success`, `-warning`, `-critical`, `-info` |
| Text | `--mm-color-text-default`, `-secondary`, `-tertiary`, `-disabled`, `-brand`, `-on-brand`, `-on-critical`, `-on-frame`, `-on-frame-active`, `-on-frame-secondary`, `-neutral`, `-success`, `-warning`, `-critical`, `-info` |
| Icon | `--mm-color-icon-default`, `-secondary`, `-disabled`, `-brand`, `-on-brand`, `-on-frame`, `-on-frame-active`, `-neutral`, `-success`, `-warning`, `-critical`, `-info` |
| Border | `--mm-color-border-default`, `-secondary`, `-strong`, `-focus`, `-critical`, `-frame` |
| Chart | `--mm-color-chart-primary`, `-secondary`, `-muted`, `-critical` |
| Space | `--mm-space-0`, `-025` (1), `-050` (2), `-100` (4), `-150` (6), `-200` (8), `-300` (12), `-400` (16), `-500` (20), `-600` (24), `-800` (32), `-1000` (40), `-1200` (48), `-1600` (64) |
| Radius | `--mm-radius-100` (4), `-150` (6), `-200` (8), `-250` (10), `-300` (12), `-400` (16), `-500` (20), `-full` |
| Shadow | `--mm-shadow-100` cards and tables, `-300` hero and menus, `-600` dialogs, side panels, toasts, `-button`, `-input` |
| Type | `--mm-text-heading-4xl … -sm`, `--mm-text-body-lg`, `-lg-medium`, `-md`, `-md-medium`, `-sm`, `-sm-medium`, `-xs`, `--mm-text-code-md`, `-sm`, `-sm-medium` (each is a `font` shorthand) |
| Sizes | `--mm-size-control-md` (32), `--mm-size-control-lg` (44), `--mm-size-icon` (16), `--mm-size-sidebar` (248), … |

Primitives (`--mm-slate-*`, `--mm-brand-*`, `--mm-green-*`, `--mm-amber-*`, `--mm-red-*`,
`--mm-cyan-*`) exist but components use the semantic tokens only. Sizes are in `rem` (1rem =
16px), so the whole interface follows the reader's font size. Raw sizes that Figma does not bind
to a variable are `--mm-size-*` tokens with their pixel value in a comment.

Breakpoints (media queries cannot read custom properties): 48rem (768px) is the mobile switch:
top bar instead of the sidebar, Large controls, one column. Three more only reflow: 64rem (KPI
cards two by two), 72rem (the Home side cards go under the main card) and 80rem (Settings
sections stack). Dark mode adds one thing Figma does not draw: a hairline around dialogs, side
panels and menus (`--mm-color-border-floating`), whose shadow cannot be seen on a dark canvas.

Text classes: `.text-heading-4xl … .text-heading-sm`, `.text-body-lg`, `.text-body-lg-medium`,
`.text-body-md`, `.text-body-md-medium`, `.text-body-sm`, `.text-body-sm-medium`, `.text-body-xs`,
`.text-code-md`, `.text-code-sm`, `.text-code-sm-medium`. Colour classes: `.text-default`,
`.text-secondary`, `.text-tertiary`, `.text-brand`, `.text-success`, `.text-warning`,
`.text-critical`, `.text-info`.

## Page shell

```html
<!doctype html>
<html lang="en">                                  <!-- data-sidebar="collapsed" from the mm_sidebar cookie -->
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>MCPs · MyMCPs</title>
  <meta name="csrf-token" content="…">
  <link rel="icon" href="/assets/brand/favicon.svg" type="image/svg+xml">
  <link rel="stylesheet" href="/assets/css/app.css">
  <script type="module" src="/assets/js/app.js" nonce="…"></script>
</head>
<body class="app">
  <a class="skip-link" href="#main">Skip to content</a>

  <header class="topbar">                         <!-- shown under 768px only -->
    <button type="button" class="icon-button icon-button--on-frame icon-button--lg" popovertarget="sidebar" aria-label="Open navigation">ICON menu</button>
    <a class="logo logo--on-frame" href="/">LOGO</a>
    <a class="topbar__account" href="/settings" aria-label="Settings"><span class="avatar avatar--on-frame">TT</span></a>
  </header>

  <aside class="sidebar" id="sidebar" popover>    <!-- a popover on mobile, a plain column on desktop -->
    <div class="sidebar__header">
      <a class="logo logo--on-frame" href="/">LOGO</a>
      <button type="button" class="icon-button icon-button--on-frame sidebar__collapse" data-sidebar-toggle aria-pressed="false" aria-label="Collapse sidebar" data-label-pressed="Expand sidebar">ICON panel-left</button>
      <button type="button" class="icon-button icon-button--on-frame sidebar__close" popovertarget="sidebar" popovertargetaction="hide" aria-label="Close navigation">ICON x</button>
    </div>
    <nav class="sidebar__nav" aria-label="Main navigation">
      <a class="nav-item" href="/" aria-current="page">ICON house<span class="nav-item__label">Home</span></a>
      <p class="nav-section">Gateway</p>
      <a class="nav-item" href="/mcps">ICON plug<span class="nav-item__label">MCPs</span></a>
      <a class="nav-item" href="/tokens">ICON key-round<span class="nav-item__label">Access tokens</span></a>
      <a class="nav-item" href="/approvals" aria-label="Approvals, 2 waiting">ICON shield-check<span class="nav-item__label">Approvals</span><span class="nav-item__count" aria-hidden="true">2</span></a>
      <p class="nav-section">Observability</p>    <!-- admins only, with Logs and Analytics -->
      <a class="nav-item" href="/logs">ICON scroll-text<span class="nav-item__label">Logs</span></a>
      <a class="nav-item" href="/analytics">ICON chart-column<span class="nav-item__label">Analytics</span></a>
      <p class="nav-section">Instance</p>
      <a class="nav-item" href="/invites">ICON users<span class="nav-item__label">Team</span></a>   <!-- admins only -->
      <a class="nav-item" href="/settings">ICON settings<span class="nav-item__label">Settings</span></a>
    </nav>
    <div class="gateway-card">
      <p class="gateway-card__status"><span class="status-dot status-dot--success"></span>Gateway online</p>
      <p class="gateway-card__endpoint"><span>mcp.example.com/mcp</span>
        <button type="button" class="gateway-card__copy" data-copy="https://mcp.example.com/mcp" aria-label="Copy gateway URL">ICON copy ICON check</button></p>
    </div>
    <div class="nav-user">
      <span class="avatar avatar--on-frame">TT</span>
      <span class="nav-user__text"><span class="nav-user__name">Thomas Tastet</span><span class="nav-user__role">admin</span></span>
      <form method="post" action="/logout"><input type="hidden" name="_csrf" value="…">
        <button type="submit" class="icon-button icon-button--on-frame" aria-label="Log out">ICON log-out</button></form>
    </div>
  </aside>

  <main class="panel" id="main">
    <div class="page">                            <!-- .page--narrow for a single column of text -->
      <!-- optional page-level banner, then the page header, then the sections, 24px apart -->
    </div>
  </main>

  <!-- dialogs and side panels of the page go here, next to <main> -->

  <div class="toast-region" data-toasts><!-- flash toasts --></div>
  <template id="toast-template">…</template>
  <template id="toast-error-template">…</template>
  <template id="confirm-template">…</template>
</body>
</html>
```

The selected entry is `aria-current="page"`. The count chip is optional (pending approvals).
On desktop the panel scrolls and the frame stays put; on mobile the document scrolls under a
sticky top bar.

Auth screens (sign in, authorize, onboarding, invite, errors without a session):

```html
<body class="auth">
  <main class="auth__main" id="main">
    <section class="auth-card control-lg">       <!-- .auth-card--wide: 520px (authorize) -->
      <span class="logo">LOGO</span>
      <div class="auth-card__heading"><h1 class="auth-card__title">Sign in</h1>
        <p class="auth-card__subtitle">Access this self-hosted MyMCPs instance. New users join by invite only.</p></div>
      <form class="form" method="post" action="/login">
        <div class="field">…</div>
        <button type="submit" class="button button--primary button--block">Sign in</button>
      </form>
    </section>
    <p class="auth__footnote">Self-hosted · invite-only · no public registration</p>
  </main>
</body>
```

## Icons and logo

Icons are Lucide SVGs inlined by the server (they take `currentColor`), 16px, 1.5px stroke at
every size:

```html
<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><!-- content of icons/house.svg --></svg>
```

Modifiers: `.icon--lg` (20px), colours `.icon--secondary`, `.icon--brand`, `.icon--success`,
`.icon--warning`, `.icon--critical`, `.icon--info`, `.icon--spin`, and `.icon--filled` for the
brand marks of `icons/brands/`.

The logo is two paths so it follows the theme:

```html
<a class="logo logo--on-frame" href="/">           <!-- without --on-frame on a light surface -->
  <svg class="logo__mark" viewBox="0 0 28 28" aria-hidden="true">
    <path class="logo__tile" d="…"/><path class="logo__letter" d="…"/>  <!-- paths of brand/logo-mark.svg -->
  </svg><span class="logo__wordmark">MyMCPs</span>
</a>
```

Icon list (Figma name `icon/<name>` = file `icons/<name>.svg`): activity, arrow-left,
arrow-right, arrow-up-right, bike, blocks, book-open, bot, braces, bug, cable, calendar,
chart-column, check, chevron-down, chevron-left, chevron-right, chevrons-up-down, circle-alert,
circle-check, circle-x, clock, cloud, copy, database, download, ellipsis, external-link, eye,
eye-off, file-text, flame, gauge, git-branch, globe, hash, house, info, key, key-round, layers,
link, list-filter, loader-circle, lock, log-out, mail, menu, minus, moon, notebook-text, package,
panel-left, pencil, play, plug, plus, power, refresh-cw, rotate-cw, scroll-text, search, send,
server, settings, shield-check, shopping-bag, sparkles, square-terminal, sun, ticket, timer,
trash, triangle-alert, unplug, user, user-plus, users, wrench, x, zap. Added for what the Figma
file does not draw: ban, credit-card, megaphone. The Figma `icon/trash` is the Lucide `trash`
of the bundled version (the bin with two lines, formerly `trash-2`).

Template icons (the Figma gallery uses Lucide icons, not brand logos): Notion `notebook-text`,
Shopify Dev `shopping-bag`, GitHub `git-branch`, Linear `layers`, Strava `bike`, iCloud Mail
`mail`, Atlassian Rovo `ticket`, Postman `send`, Sentry `bug`, Context7 `braces`; chosen for the
templates Figma does not draw: Microsoft Learn `book-open`, Google Ads `megaphone`, Stripe
`credit-card`, Supabase `zap`, Cloudflare Docs `cloud`, Firebase `flame`, MongoDB `database`,
Neon `server`, Hugging Face `bot`. An MCP that matches no template takes `plug`.

## Components

### Button

```html
<button type="submit" class="button button--primary">ICON plus Add MCP</button>
<a class="button button--secondary" href="/logs">View logs</a>
<button type="button" class="button">Edit</button>                      <!-- tertiary -->
<button type="submit" class="button button--critical">Delete</button>
<button class="button button--primary button--lg button--block">Sign in</button>
```

Variants: `--primary`, `--secondary`, `--critical`, none (tertiary). Sizes: `--lg`, `--md`.
`--block` is full width. `disabled` or `aria-disabled="true"` greys it. `aria-busy="true"`
(set by app.js while a form is sent) shows a spinner in place of the icon.

```html
<button type="button" class="icon-button" aria-label="More actions">ICON ellipsis</button>
```

Icon button variants: none (tertiary), `--secondary`, `--on-frame`; size `--lg` (44px).

### Badge, status, code, kbd, avatar, tile

```html
<span class="badge badge--success">Active</span>            <!-- 6px dot by default -->
<span class="badge badge--info badge--no-dot">Admin only</span>
<span class="status status--critical">error</span>          <!-- table cells -->
<span class="status-dot status-dot--success"></span>
<code class="code">mcp:tools</code>   <code class="code code--info">mcp:tools</code>
<kbd class="kbd">⌘K</kbd>
<span class="avatar">TT</span> <span class="avatar avatar--md">TT</span> <span class="avatar avatar--on-frame">TT</span>
<span class="tile">ICON plug</span> <span class="tile tile--lg">ICON notebook-text</span>
```

Badge tones: `--neutral` (default), `--success`, `--warning`, `--critical`, `--info`, `--brand`.
Status tones: neutral (default), `--success`, `--warning`, `--critical`.

### Banner, empty state, toast

```html
<div class="banner banner--warning" role="status">
  ICON(triangle-alert)
  <div class="banner__content"><p class="banner__title">Set APP_URL to enable public links</p>
    <p>Define APP_URL as this instance’s public HTTPS origin, then redeploy.</p></div>
  <a class="button button--secondary" href="…">Action</a>           <!-- optional -->
</div>
```

Tones and their icon: none = info (`info`), `--success` (`circle-check`), `--warning`
(`triangle-alert`), `--critical` (`circle-x`). Use `role="alert"` for errors.

```html
<div class="empty-state">
  <span class="empty-state__icon">ICON search</span>
  <div class="empty-state__text"><p class="empty-state__title">No calls in this view</p>
    <p class="empty-state__description">Change the filters or make a tool call through the MCP gateway.</p></div>
  <a class="button button--secondary" href="/logs">Clear filters</a>
</div>
```

```html
<div class="toast-region" data-toasts>
  <div class="toast" data-toast role="status">ICON circle-check<p class="toast__message">MCP created</p>
    <button type="button" class="toast__dismiss" data-toast-dismiss aria-label="Dismiss">ICON x</button></div>
  <div class="toast toast--error" data-toast role="alert">ICON circle-x<p class="toast__message">…</p>…</div>
</div>
```

Info toasts hide after 5 s, error toasts stay. The three `<template>` elements of the shell hold
one info toast, one error toast and the confirm dialog, for app.js to clone.

### Fields

```html
<div class="field">
  <label class="field__label" for="name">Name <span class="field__optional">Optional</span></label>
  <input class="input" id="name" name="name" type="text" aria-describedby="name-help">
  <p class="field__help" id="name-help">Shown in the MCP list.</p>
</div>

<!-- error -->
<input class="input" id="url" name="url" aria-invalid="true" aria-describedby="url-error">
<p class="field__error" id="url-error">Enter an https URL.</p>

<!-- select: a field with the chevron -->
<div class="select"><select class="input" id="level" name="level">…</select>ICON chevron-down</div>

<!-- textarea -->
<textarea class="input" id="description" name="description" rows="3"></textarea>

<!-- leading icon, trailing action -->
<div class="input-group">
  <input class="input-group__control" id="password" name="password" type="password">
  <button type="button" class="input-group__action" data-password-toggle="#password" aria-pressed="false"
          aria-label="Show password" data-label-pressed="Hide password">ICON eye ICON eye-off</button>
</div>

<!-- search -->
<label class="input-group search"><span class="visually-hidden">Search MCPs</span>ICON search
  <input class="input-group__control" type="search" name="q" placeholder="Search MCPs" data-shortcut="mod+k">
  <kbd class="kbd" data-shortcut-hint>⌘K</kbd></label>
```

`.input--mono` for identifiers. `.search--on-frame` on the dark frame. Two fields side by side:
`<div class="grid grid--2">`. A form is `<form class="form">` (blocks 16px apart).

```html
<!-- a number and its unit -->
<label class="input-group"><input class="input-group__control" type="number" name="retentionDays" value="14" min="1" max="365">
  <span class="input-group__unit">days</span></label>

<!-- repeatable rows (environment variables) -->
<fieldset class="field-group" data-repeat>
  <legend class="field-group__label">Environment variables</legend>
  <div class="repeat-list" data-repeat-list>
    <div class="repeat-row" data-repeat-row>
      <div class="field">…name…</div><div class="field">…value…</div>
      <button type="button" class="button" data-repeat-remove>Remove</button></div>
  </div>
  <template data-repeat-template>…the same row with __i__ in name, id and for…</template>
  <div class="cluster"><button type="button" class="button button--secondary" data-repeat-add>Add variable</button></div>
</fieldset>
```

```html
<label class="choice"><input type="checkbox" class="checkbox" name="enabled" checked>
  <span class="choice__text"><span class="choice__label">Enabled</span>
    <span class="choice__description">Disabled MCPs are excluded from the gateway</span></span></label>
<label class="choice"><input type="radio" class="radio" name="scope" value="all"><span class="choice__label">All MCPs</span></label>
<input type="checkbox" class="switch" role="switch" name="lazy" aria-label="Enable lazy tool mode">
<!-- a switch that submits its own form (table rows) -->
<form method="post" action="/mcps/3/toggle"><input type="hidden" name="_csrf" value="…">
  <button type="submit" class="switch" role="switch" aria-checked="true" aria-label="Enabled"></button></form>
```

```html
<fieldset class="field-group">
  <legend class="field-group__label">Transport</legend>
  <p class="field-group__help">Optional line, above the control.</p>
  <div class="option-cards">
    <label class="option-card"><input type="radio" class="radio" name="transport" value="http" checked>
      <span class="option-card__text"><span class="option-card__title">HTTP</span>
        <span class="option-card__description">Remote Streamable HTTP URL</span></span></label>
    …
  </div>
</fieldset>

<label class="setting-row">
  <span class="setting-row__text"><span class="setting-row__title">Enable lazy tool mode</span>
    <span class="setting-row__description">Adds X-MyMCPs-Tool-Mode: lazy so clients discover tools on demand.</span></span>
  <input type="checkbox" class="switch" role="switch" name="lazy">
</label>
```

### Chips, segments, tabs

```html
<!-- filter chip: opens a menu; brand tint and a clear link once a value is set -->
<button type="button" class="chip" popovertarget="filter-status">Status ICON chevron-down</button>
<span class="chip chip--active"><button type="button" class="chip__label" popovertarget="filter-outcome">Outcome: error</button>
  <a class="chip__clear" href="/logs" aria-label="Clear outcome filter">ICON x</a></span>

<!-- segments: links, buttons (role="tab"), or radios -->
<div class="segmented">
  <a class="segment" href="?range=24h">24 hours</a>
  <a class="segment" href="?range=7d" aria-current="true">7 days</a>
</div>
<div class="segmented segmented--block">…</div>          <!-- full width, equal segments -->

<div class="tabs" data-tabs>
  <div class="tabs__list" role="tablist" aria-label="Client">
    <button type="button" class="tab" role="tab" id="t-codex" aria-controls="p-codex" aria-selected="true">Codex</button>
    <button type="button" class="tab" role="tab" id="t-claude" aria-controls="p-claude" aria-selected="false" tabindex="-1">Claude</button>
  </div>
  <div role="tabpanel" id="p-codex" aria-labelledby="t-codex">…</div>
  <div role="tabpanel" id="p-claude" aria-labelledby="t-claude" hidden>…</div>
</div>
```

### Page header, breadcrumb, toolbar, pagination

```html
<nav class="breadcrumb" aria-label="Breadcrumb"><a href="/mcps">MCPs</a>ICON chevron-right<span aria-current="page">Notion</span></nav>

<header class="page-header">
  <div class="page-header__text"><h1 class="page-header__title">MCPs</h1>
    <p class="page-header__subtitle">Register upstream MCP servers. Agents reach them through MyMCPs with an access token.</p></div>
  <div class="page-header__actions"><a class="button button--primary" href="/mcps/new">ICON plus Add MCP</a></div>
</header>

<div class="toolbar">                              <!-- .toolbar--tight: 8px gaps (chips only) -->
  <label class="input-group search">…</label>
  <button type="button" class="chip" popovertarget="…">Status ICON chevron-down</button>
  <p class="toolbar__note push-end">8 MCPs · 7 enabled</p>
</div>

<nav class="pagination" aria-label="Pagination">
  <span class="pagination__range">1–25 of 1,284</span>
  <span class="pagination__buttons">
    <a class="icon-button icon-button--secondary" href="?page=1" aria-label="Previous page">ICON chevron-left</a>
    <a class="icon-button icon-button--secondary" aria-disabled="true" aria-label="Next page">ICON chevron-right</a>
  </span>
</nav>
```

### Card, list rows, key values

```html
<section class="card">
  <header class="card__header">
    <div class="card__heading"><h2 class="card__title">Gateway activity</h2>
      <p class="card__subtitle">Tool calls · last 14 days</p></div>
    <a class="button" href="/logs">Open logs</a>                    <!-- optional, one link -->
  </header>
  <a class="list-row" href="…">                     <!-- or <div class="list-row"> -->
    <span class="tile">ICON square-terminal</span>
    <span class="list-row__text"><span class="list-row__title list-row__title--code">notion__search</span>
      <span class="list-row__subtitle">Notion · Cursor agent</span></span>
    <span class="list-row__meta">2 min ago</span>
    <span class="badge badge--success">Success</span>
  </a>
  <div class="card__body">…free content, 20px padding…</div>
  <footer class="card__footer"><button class="button button--primary">Save instance settings</button></footer>
</section>

<div class="heading"><h2 class="heading__title">Gateway URL</h2>
  <p class="heading__description">MCP clients connect to this URL.</p></div>

<div class="kpi"><p class="kpi__label">MCPs</p><p class="kpi__value">8</p>
  <p class="kpi__footer"><span class="badge badge--success badge--no-dot">+2</span><span>this month</span></p></div>
<div class="metric"><p class="metric__label">Success rate</p><p class="metric__value">98.6%</p></div>
<dl class="key-values"><div class="key-value"><dt>Requested tool</dt><dd>sentry__find_issues</dd></div>…</dl>

<div class="detail-row">
  <div class="detail-row__text"><span class="detail-row__label">Email</span><span class="detail-row__value">thomas@tastet.dev</span></div>
  <button type="button" class="button button--secondary" data-dialog-open="#change-email">Change email</button>
</div>
```

`.list-row__title--code` for tool names; plain titles are Inter. KPI cards go in
`<div class="grid grid--4 grid--keep-2">`. More:

- `.list-row--wrap`: the title is a sentence and wraps; put the badge and the meta in a
  `<span class="list-row__footer">` inside `.list-row__text`, and end the row with a chevron icon.
- `.list-row__meta--slot`: a note where the other rows have an icon button ("You").
- `<dd class="key-value__before">Now: €2.50 a day</dd>`: the value a change replaces.
- `.card__footer-note`: a note at the start of a card footer ("2 unsaved changes").
- `.breadcrumb--truncate`: one line, the current page is cut with an ellipsis.
- `.tile--critical`, `.tile--round`, `<span class="tile"><img src="…" alt=""></span>`.
- `<dl class="card metric-strip">` of `.metric` (`<dt class="metric__label">`, `<dd class="metric__value">`):
  metrics side by side in one card with dividers (Analytics).

### Table

A card that holds a real `<table>`. The layout is fixed: give the fixed columns their Figma
width with `.col-<px>` on the `<th>` (40 to 240, step 4); the columns without a class share the
rest equally and truncate with an ellipsis.

```html
<div class="card">
  <div class="table-scroll">
    <table class="table">                           <!-- --dense: 48px rows (logs); --tight: 44px; --interactive: row hover -->
      <thead><tr>
        <th class="col-88">Status</th><th>Name</th><th>Endpoint</th>
        <th class="col-96">Auth</th><th class="col-72">Enabled</th><th class="col-96 cell-end">Actions</th>
      </tr></thead>
      <tbody><tr>
        <td><span class="status status--success">ready</span></td>
        <td><div class="cell-media"><span class="tile">ICON notebook-text</span>
          <div class="cell-stack"><span class="cell-title">Notion</span><span class="cell-sub cell-sub--code">notion</span></div></div></td>
        <td><div class="cell-stack"><span class="cell-code">https://mcp.notion.com/mcp</span>
          <span class="cell-sub cell-sub--critical">OAuth token was rejected (HTTP 401)</span></div></td>
        <td><code class="code">auto</code></td>
        <td><form …><button class="switch" role="switch" aria-checked="true" aria-label="Enabled"></button></form></td>
        <td class="cell-end"><span class="row-actions"><a class="button" href="/mcps/1/edit">Edit</a>
          <button type="button" class="icon-button" popovertarget="mcp-1-menu" aria-label="More actions">ICON ellipsis</button></span></td>
      </tr></tbody>
    </table>
  </div>
  <div class="table-footer"><p class="table-footer__note">25 rows per page</p><nav class="pagination">…</nav></div>
</div>
```

Cell classes on `<td>`: `.cell-end`, `.cell-secondary`, `.cell-tertiary`, `.cell-strong`,
`.cell-code`, `.cell-code-strong`, `.cell-wrap`. In a cell: `.cell-media` (tile or avatar and
text), `.cell-stack` (two lines; `--wrap` when the first is a sentence), `.cell-title`,
`.cell-sub` (`--code`, `--critical`, `--secondary`), `.row-actions`. Selected row:
`<tr aria-selected="true">`, or any row that holds an `aria-current="true"` trigger. Under a
`.card__header` the column titles are 36px high. An empty table is an `.empty-state` in place of
the `.table-scroll`.

- A table is never narrower than 680px: below that it scrolls inside its card. `.table--wide`
  keeps 896px (a sentence in a cell). `.table--tight` is the ranking table of Analytics (44px
  rows, 12px between columns, no minimum width). `.table--fluid` drops the minimum under 768px.
- Rows that open something: `class="table table--interactive"` and one
  `<a class="row-link" href="…">` in a cell; its hit area and focus ring are the whole row.

On mobile a table scrolls inside its card. For the main lists, render a list too and switch with
`.hide-mobile` / `.hide-desktop`:

```html
<div class="card hide-mobile">…table…</div>
<section class="card hide-desktop">
  <header class="card__header"><div class="card__heading"><h2 class="card__title">Registered MCPs</h2>
    <p class="card__subtitle">8 MCPs · 7 enabled</p></div></header>
  <a class="list-row" href="/mcps/1/edit"><span class="tile">ICON notebook-text</span>
    <span class="list-row__text"><span class="list-row__title">Notion</span><span class="list-row__subtitle">notion · HTTP</span></span>
    <span class="badge badge--success">ready</span></a>
</section>
```

### Code block, copy field, steps

```html
<div class="code-block">
  <div class="code-block__header"><span class="code-block__title">~/.cursor/mcp.json</span>
    <button type="button" class="icon-button" data-copy-target="#cursor-config" aria-label="Copy">ICON copy ICON check</button></div>
  <pre class="code-block__body" id="cursor-config">{ … }</pre>
</div>

<div class="copy-field"><span class="copy-field__value" id="gateway-url">https://mcp.example.com/mcp</span>
  <button type="button" class="icon-button" data-copy-target="#gateway-url" aria-label="Copy gateway URL">ICON copy ICON check</button></div>

<ol class="steps"><li class="step">Restart Claude Code after the command completes.</li><li class="step">…</li></ol>
```

`.copy-field--lg` is the 48px field of the Home hero. `.copy-field--wrap` wraps a value that must
be read whole (a token shown once, a redirect URI).

```html
<!-- steps that hold content: a setup guide -->
<ol class="steps steps--detailed" aria-label="Strava setup">
  <li class="step" data-complete><div class="step__body"><h3 class="step__title"><span class="visually-hidden">Done: </span>Create a Strava API application</h3>…</div></li>
  <li class="step" aria-current="step"><div class="step__body"><h3 class="step__title">Paste its Client ID and Client Secret</h3>…fields…</div></li>
</ol>

<!-- a link in running text -->
<a class="link" href="https://…" target="_blank" rel="noopener noreferrer">Strava API settings ICON(external-link)</a>
```

### Template card

```html
<article class="template-card" data-filter-item data-filter-text="notion productivity notes wiki" data-filter-tags="popular productivity">
  <div class="template-card__header"><span class="tile tile--lg">ICON notebook-text</span>
    <div class="template-card__text"><h3 class="template-card__name">Notion</h3><p class="template-card__category">Productivity</p></div>
    <span class="badge badge--info badge--no-dot">Built-in</span></div>      <!-- optional -->
  <p class="template-card__description">Search, read, and update pages and databases in your Notion workspace.</p>
  <a class="button button--secondary" href="/mcps/new?template=notion">Set up Notion</a>
</article>
```

The gallery is `<div class="grid grid--3">` of template cards.

### Dialog

```html
<a class="button button--primary" href="/mcps/new" data-dialog-open="#add-mcp">ICON plus Add MCP</a>

<dialog class="dialog dialog--xl" id="add-mcp" aria-labelledby="add-mcp-title">   <!-- data-open: opened on load -->
  <div class="dialog__header">
    <div class="dialog__heading"><h2 class="dialog__title" id="add-mcp-title">Add an MCP</h2>
      <p class="dialog__subtitle">Start from a trusted template or configure your own server</p></div>
    <button type="button" class="icon-button" data-dialog-close aria-label="Close">ICON x</button>
  </div>
  <div class="dialog__body">…</div>
  <div class="dialog__footer">
    <button type="submit" class="button button--critical dialog__footer-start" form="delete-mcp">Delete</button>   <!-- optional, left -->
    <button type="button" class="button button--secondary" data-dialog-close>Cancel</button>
    <button type="submit" class="button button--primary">Save changes</button>
  </div>
</dialog>
```

Widths: `--sm` 480, none 640, `--lg` 680, `--xl` 896. The body scrolls between the header and the
footer. A form may wrap header, body and footer: `<dialog class="dialog"><form method="post" …
data-async>…</form></dialog>`; with async forms wrap it in `<div data-fragment>`. On mobile a
dialog fills the screen width.

### Side panel

```html
<dialog class="drawer" id="call-details" data-dialog-modal="false" aria-labelledby="call-title">
  <div class="drawer__header">
    <div class="drawer__heading"><h2 class="drawer__title" id="call-title">Call details</h2><p class="drawer__meta"><time datetime="…">…</time></p></div>
    <span class="badge badge--critical">Error</span>
    <button type="button" class="icon-button" data-dialog-close aria-label="Close">ICON x</button>
  </div>
  <div class="drawer__body">…</div>
  <div class="drawer__footer"><a class="button button--secondary" href="…">Open MCP</a>…</div>
</dialog>
```

It floats 24px from the top and the right, does not dim the page, and the page stays usable.

### Menu

```html
<button type="button" class="icon-button" popovertarget="mcp-1-menu" aria-label="More actions">ICON ellipsis</button>
<div class="menu" id="mcp-1-menu" popover role="menu">       <!-- .menu--start: left edges aligned (under a chip) -->
  <a class="menu__item" role="menuitem" href="/mcps/1/tools">ICON shield-check Tool approvals</a>
  <a class="menu__item" role="menuitemradio" aria-checked="true" href="?status=ready">ready</a>
  <div class="menu__separator"></div>
  <form method="post" action="/mcps/1?_method=DELETE" data-confirm="Delete Notion?" data-confirm-label="Delete" data-confirm-tone="critical">
    <input type="hidden" name="_csrf" value="…">
    <button type="submit" class="menu__item menu__item--critical" role="menuitem">ICON trash Delete</button></form>
</div>
```

A native popover: it opens without JavaScript. app.js adds arrow keys and the fallback position.
`<p class="menu__label">` titles a group. A menu of a filter form holds radios instead of links:
`<label class="menu__item"><input type="radio" class="visually-hidden" name="outcome" value="error">Error</label>`.

### Home and settings blocks

```html
<div class="context-bar">
  <button type="button" class="range" popovertarget="range-menu">Last 7 days ICON chevron-down</button>
  <dl class="stats"><div class="stat"><dt class="stat__label">Tool calls</dt><dd class="stat__value">8,412</dd></div>…</dl>
</div>

<section class="hero">
  <h1 class="hero__greeting">Good afternoon, Thomas.<span>8 MCPs behind one endpoint.</span></h1>
  <div class="hero-card">
    <div class="hero-card__row"><span>Gateway endpoint</span><span class="badge badge--success">Online</span></div>
    <div class="copy-field copy-field--lg">…</div>
    <div class="hero-card__row"><div class="cluster">chips…</div><a class="button button--primary" …>ICON download Install in a client</a></div>
  </div>
  <div class="attention"><a class="attention__item" href="…">ICON triangle-alert Strava needs authorization ICON chevron-right</a>…</div>
</section>

<div class="overview"><section class="card">…</section><div class="overview__side"><section class="card">…</section>…</div></div>

<div class="chart-section">
  <p class="chart-total"><span class="chart-total__value">8,412</span><span class="badge badge--success badge--no-dot">+12%</span>including 118 errors</p>
  <div class="bar-chart" role="img" aria-label="Tool calls per day, last 14 days">
    <span class="bar-chart__bar" data-pct="38"></span>…<span class="bar-chart__bar" data-pct="100"></span></div>
  <div class="chart-axis"><span>24 Sep</span><span>30 Sep</span><span>7 Oct</span></div>
</div>

<!-- line chart: an inline <svg> written by the server; all text stays HTML -->
<div class="line-chart">
  <div class="line-chart__y" aria-hidden="true"><span>2,000</span><span>1,500</span><span>1,000</span><span>500</span><span>0</span></div>
  <div class="line-chart__canvas">
    <svg class="line-chart__svg" viewBox="0 0 6 2000" preserveAspectRatio="none" role="img" aria-label="Calls and errors per day">
      <path class="line-chart__grid" d="M0 0H6M0 500H6M0 1000H6M0 1500H6M0 2000H6"/>
      <g class="line-chart__series">
        <path class="line-chart__area" d="…"/><path class="line-chart__line" d="…"/><path class="line-chart__point" d="M0 1020h0"/>…</g>
      <g class="line-chart__series line-chart__series--critical">…</g>
      <rect class="line-chart__hit" x="-0.5" y="0" width="1" height="2000"><title>Oct 1: 980 calls, 12 errors</title></rect>…
    </svg>
  </div>
  <div class="line-chart__x" aria-hidden="true"><span>Oct 1</span><span>Oct 2</span>…</div>     <!-- one span per bucket; empty = no label -->
  <ul class="line-chart__legend"><li class="line-chart__key">Calls</li><li class="line-chart__key line-chart__key--critical">Errors</li></ul>
</div>

<section class="section">                           <!-- settings: intro on the left, card on the right -->
  <div class="section__intro"><h2 class="section__title">My Instance <span class="badge badge--info badge--no-dot">Admin only</span></h2>
    <p class="section__description">Configure settings that apply to everyone using this MyMCPs instance.</p></div>
  <div class="card">…</div>
</section>
```

## Layout helpers

`.stack` (column, 16px), `.cluster` (wrapping row, 8px; `--nowrap`, `--between`, `--end`, `--top`),
`.grid` with `.grid--2`, `.grid--3`, `.grid--4` (one column under 768px; `.grid--keep` keeps two,
`.grid--keep-2` makes four into two under 1024px) and `.grid--fit` (as many 320px columns as
fit), gaps `.gap-0 … .gap-800`, `.grow`, `.shrink-0`, `.push-end`, `.full-width`, `.align-end`,
`.align-center`, `.truncate`, `.clamp-3`, `.break-anywhere`, `.preserve-lines` (what an agent
sent, shown as it is), `.tabular`, `.visually-hidden`, `.hide-mobile`, `.hide-desktop`, `.well`
(tinted box), `.control-lg` / `.control-md` (control size of a region). On `.page`:
`.page--narrow` (720px column), `.page--center` (one centred block), and `.page__body` (a
wrapper that live refresh swaps without changing the page rhythm).

## Dynamic values

No inline styles. Two generated families cover data-driven sizes:

- `.col-<px>` on a `<th>`: fixed column width.
- `data-pct="0..100"` (an integer): sets `--pct`, used by `.bar-chart__bar` for its height.

Line charts are inline `<svg>` drawn by the server (geometry in attributes, colours from classes):
the `viewBox` is in data units (x = bucket index, y = 0 at the top to the axis maximum at the
bottom) and stretched; strokes keep their pixel width. The curve is a uniform Catmull-Rom spline
through the points (control points `P1 + (P2 − P0) / 6` and `P2 − (P3 − P1) / 6`, clamped to the
plot), the area is the same path closed on the baseline, and a point is a zero-length segment
(`M x y h0`) with a round cap. The axis maximum is four "nice" steps. The comment above
`.line-chart` in `app.css` gives the whole recipe.
The only custom properties app.js sets are `--popover-top` and `--popover-left` on a menu when the
browser has no CSS anchor positioning.

## JavaScript (`app.js`)

One file, no dependency, no `import` or `export`: `<script type="module" src="/assets/js/app.js"
nonce="…">` in the app (a classic `<script defer>` also works, which is how the prototypes open
from disk). Everything is declared in the HTML; HTML inserted later works without re-binding.
Global: `window.MyMCPs = { enhance(root), toast(message, { tone, sticky }), confirm(message, options) }`.
Browser floor: Chrome 126, Firefox 126, Safari 18.

| Attribute | Behaviour |
| --- | --- |
| `data-dialog-open="#id"` | Opens the `<dialog>` (`showModal()`). On a link the click is intercepted; without JS the link navigates. |
| `data-dialog-fetch` (on an `<a href>`, or `="/url"` on a button) | GETs the URL, puts the HTML in the dialog's `[data-fragment]` (else the dialog), then opens. A second trigger for an open dialog swaps in place. `data-dialog-history`: the URL becomes the address. |
| `data-dialog-close` | Closes the closest dialog. Escape and a backdrop click close too. |
| on a dialog: `data-open` | Opened on load (server-driven dialogs); `data-dialog-trigger="#id"` names the trigger that gets focus back. |
| on a dialog: `data-dialog-modal="false"` | Opened with `show()`: side panels. The page stays usable, Escape still closes. |
| on a dialog: `data-dialog-return="/path"` | `history.replaceState` to that URL when it closes (also accepted on the trigger). |
| on a dialog: `data-dialog-static`, `data-dialog-reset` | A backdrop click does not close it; its forms reset when it closes. |
| `popovertarget="id"` + `<div popover class="menu" role="menu">` | Native menu; app.js adds `aria-expanded`, arrow keys, Home, End, closing on activation, and the fallback position. |
| `data-tabs` | Tabs: `role="tab"` + `aria-controls`; `data-tabs="hash"` keeps the tab in the URL hash. |
| `data-toggle="#id"`, `data-reveal="#id"` | Toggles `hidden` on the target (and `aria-expanded`); reveal shows it without cancelling the click. `<details>` needs nothing. |
| `data-copy="text"` / `data-copy-target="#id"` | Copies; the trigger gets `data-copied` for 2 s. Put two icons in it (copy, check), or a `<span data-copy-label data-copied-label="Copied">`. |
| `data-toasts`, `data-toast`, `data-toast-dismiss`, `data-toast-sticky` | Flash toasts: info hides after 5 s, `.toast--error` and sticky ones stay. |
| `form[data-async]` inside `[data-fragment]` | Sent with `fetch`; see the protocol below. `data-async-target="#id"`, `data-async-history` (GET). |
| `form[data-autosubmit]` | Submits when a named control changes, and 300 ms after typing in a text field. |
| `data-busy-message="Testing…"` on a plain form or its submit button | A toast shown from the submit until the answer replaces the page: for the posts that take seconds (testing a connection, updating an npm MCP). |
| `data-confirm="Message"` on a form or a submit button | Styled confirm dialog. `data-confirm-title`, `data-confirm-label`, `data-confirm-tone="critical"`; `{count}` = checked `[data-select-item]`. |
| `data-show-when="name=value"`, `data-hide-when`, `data-optional-when` | Conditional blocks: `name=a\|b`, `name!=a`, `name=on` / `off` for a checkbox, `name=` for empty, `&` to combine, `name:origin=https://host`. Hidden controls are disabled. |
| `data-repeat`, `-list`, `-row`, `-template`, `-add`, `-remove`, `-min`, `-max` | Repeatable rows; `__i__` in the template is the row index. |
| `<time datetime="…">` | Rewritten in the viewer's time zone: `dd/mm/yyyy, hh:mm:ss`. `data-format="minute"`: `dd/mm/yyyy, hh:mm` (tables); `"date"`: `dd/mm/yyyy`; `"relative"`: `2 min ago`. |
| `data-timezone-label`, `input[data-timezone]`, `form[data-timezone-sync]` | The viewer's time zone as text (`Europe/Paris (UTC+02:00)`), as a field value, and one resubmit when it differs from the server's. |
| `<input type="datetime-local" data-utc="ISO">` | Shown in local time, submitted as a UTC instant. |
| `data-filter`, `-input`, `-value`, `-item`, `-text`, `-tags`, `-count`, `-empty`, `-query`, `-clear` | Client-side filter of a list (template gallery, tool list). Counts take `data-singular`, `data-plural`, `data-zero`. |
| `data-live-refresh="#id"`, `data-live-interval="30000"` | A pressed toggle that re-fetches the page and swaps the target every interval; remembered per page for the session. |
| `data-password-toggle="#id"` | Show or hide a password; `data-label-pressed` is the label while shown. Two icons (eye, eye-off). |
| `data-bind-source="key"`, `data-bind="key"`, `data-bind-empty`, `data-bind-format="json\|shell"`, `data-bind-scope` | Mirrors what is typed into text elements (the token in the install snippets), escaped for its place. |
| `data-select-all="#id"`, `data-select-item`, `data-select-count="#id"`, `data-select-bar="#id"`, `data-select-empty="#id"` | Bulk selection in a table. |
| `data-check-all="#id"` + `data-check-value` | Checks every visible radio or checkbox carrying that value ("All ask"). |
| `form[data-dirty]`, `data-dirty-count`, `data-dirty-submit` | Counts changed fields; the submit stays disabled until something changed. |
| `data-sidebar-toggle` | Collapses the desktop sidebar: `<html data-sidebar="collapsed">`, kept in `localStorage` and in the cookie `mm_sidebar=collapsed` so the server can render the attribute. |
| `data-shortcut="mod+k"`, `data-shortcut-hint` | Cmd/Ctrl+K focuses the field; the hint shows the platform's spelling. |
| `data-oauth-paste`, `data-oauth-paste-submit`, `data-oauth-paste-error` | Finishes OAuth from a pasted loopback address. |

State app.js writes, already styled: `html[data-dialog-open]`, `html[data-sidebar="collapsed"]`,
`aria-current="true"` on the trigger of an open dialog, `aria-busy="true"` on a pending form and
its submit button, `aria-expanded`, `aria-selected`, `aria-pressed`, `data-copied`,
`data-copy-failed`, `data-leaving` (toast), `data-dirty="true|false"`, `hidden`, `disabled`.
A dialog with no `[autofocus]` and no invalid field takes the focus itself: put `autofocus` on
the first field of a form dialog. Events (bubbling): `app:enhance`, `app:dialog-open`,
`app:dialog-close`, `app:async-success`, `app:async-error`.

### Async form protocol

```html
<div data-fragment id="mcp-form">
  <form method="post" action="/mcps/3?_method=PUT" data-async>
    <input type="hidden" name="_csrf" value="…">
    …
    <button type="submit" class="button button--primary">Save changes</button>
  </form>
</div>
```

Request: same URL (query string kept, so `?_method=PUT` survives), method and body as the native
submit (`application/x-www-form-urlencoded`, or `multipart/form-data` when the form says so; a
GET form puts its fields in the query), plus the headers `X-Requested-With: fetch`,
`Accept: text/html`, `X-Fragment: <id of the element that will receive the HTML>` and, on
non-GET requests, `X-CSRF-Token` from `<meta name="csrf-token">`. Dialog fetches and live
refreshes send the same headers.

Response, in this order:

1. Header `X-Location: /path`: the browser navigates there. Use it instead of a 302 (a redirect
   that was followed anyway also navigates).
2. Status 200 to 299 or 422 with a `text/html` body: the content of the closest `[data-fragment]`
   (or of `data-async-target`) is replaced by the body and enhanced again. 422 is for validation
   errors: render the same form with `aria-invalid="true"` and `.field__error`. A fragment may
   carry `[data-toast]` elements (they move to the toast region) and a `dialog[data-open]`.
3. Status 204: nothing changes.
4. Anything else: the error toast `Something went wrong. Try again.`, the form stays as it was.

While the request runs the form and its submit button have `aria-busy="true"` and every submit
button of the form is disabled. Without JavaScript the same form posts normally, so the handler
must also answer a plain request (redirect or full page). A live refresh is a plain GET of the
current URL: the element with the target's `id` is taken from the full page that comes back.

## Do and do not

- Do render dialogs server-side with `data-open` when the URL means "this dialog is open"
  (`/mcps/3/edit`), and give them `data-dialog-return`.
- Do put the two icons (`copy`, `check`) in every copy button, and (`eye`, `eye-off`) in every
  password toggle: CSS swaps them.
- Do write dates as `<time datetime="2026-10-07T12:21:09Z">07/10/2026, 12:21:09</time>` with the
  UTC text as the fallback.
- Do not use `style=""`, `<style>`, inline event handlers, `window.confirm`, or icon fonts.
- Do not colour-code with raw colours: pick a tone (`success`, `warning`, `critical`, `info`,
  `brand`, `neutral`).
- Do not put a card inside a card. Use `.well`, `.setting-row` or `.code-block` inside a card.
- Do not make row actions critical buttons: "Revoke", "Delete" and "Remove" in a row are tertiary
  buttons with a confirm prompt; the critical button is for the footer of a dialog.
