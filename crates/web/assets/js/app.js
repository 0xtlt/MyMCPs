/*
 * MyMCPs client behaviour for the server-rendered pages: no dependency, no build step, no import
 * or export (the same file runs as a module and as a classic deferred script). Behaviours are
 * opt-in through the attributes below, and state goes into attributes that app.css styles.
 *
 * Attributes read
 *   data-dialog-open="#id"          trigger: opens that <dialog>
 *   data-dialog-fetch[="/url"]      trigger: loads the dialog content first (default URL: href)
 *   data-dialog-history             trigger: the loaded URL becomes the address
 *   data-dialog-close               closes the closest dialog
 *   data-open                       dialog: opened as soon as it is enhanced (then removed)
 *   data-dialog-modal="false"       dialog: opened with show(), the page stays usable
 *   data-dialog-static              dialog: a backdrop click does not close it
 *   data-dialog-return="/path"      dialog or trigger: address restored when the dialog closes
 *   data-dialog-trigger="#id"       dialog rendered open: the trigger that gets focus back
 *   data-dialog-reset               dialog: its forms and password toggles reset when it closes
 *   data-fragment                   element whose content the server re-renders
 *   data-tabs[="hash"]              tab group; "hash" mirrors the selected panel in the address
 *   data-toggle="#id"               button: shows or hides the target
 *   data-reveal="#id"               shows the target, without cancelling the click
 *   data-copy="text"                copies the literal text
 *   data-copy-target="#id"          copies the value or the text of the target
 *   data-copy-label                 element in a copy trigger whose text changes for 2 s
 *   data-copied-label="Copied"      on it: text shown after a copy
 *   data-toasts                     region that holds the toasts
 *   data-toast                      one toast; moved into the region
 *   data-toast-sticky               toast that stays until dismissed
 *   data-toast-dismiss              button that removes its toast
 *   data-async                      form sent with fetch, its fragment replaced by the response
 *   data-async-target="#id"         form or submitter: element replaced instead of the fragment
 *   data-async-history              GET form: the requested URL becomes the address
 *   data-autosubmit                 form submitted when one of its named controls changes
 *   data-busy-message="Testing…"    plain form or submitter: toast shown while its answer is awaited
 *   data-busy-label="Importing…"    submit button of a plain form: its text while the answer is awaited
 *   data-download                   plain form answered with a file: it resets and closes its dialog
 *   data-confirm="Message"          form or submitter: asks before submitting ({count} allowed)
 *   data-confirm-title              title of that prompt
 *   data-confirm-label              label of its accept button
 *   data-confirm-tone="critical"    accept button drawn as destructive
 *   data-confirm-title-slot         in #confirm-template: receives the title
 *   data-confirm-message-slot       in #confirm-template: receives the message
 *   data-confirm-accept             in #confirm-template: the accept button
 *   data-show-when="name=value"     visible only while the condition holds
 *   data-hide-when="name=value"     hidden while the condition holds
 *   data-optional-when="name=value" control required unless the condition holds
 *   data-repeat                     group of repeatable rows
 *   data-repeat-min, -max           bounds of the number of rows
 *   data-repeat-list                element that holds the rows
 *   data-repeat-row                 one row
 *   data-repeat-template            <template> of a row, __i__ standing for its index
 *   data-repeat-add                 button that appends a row
 *   data-repeat-remove              button that deletes its row
 *   data-format="date|minute|datetime|relative"  on <time datetime>: how the local time is written
 *   data-timezone-label             text replaced by the viewer's time zone and offset
 *   data-timezone                   input whose value becomes the viewer's time zone
 *   data-timezone-sync              form submitted once when that value changed
 *   data-utc="ISO"                  datetime-local input shown in local time, submitted in UTC
 *   data-filter                     client-side filter group
 *   data-filter-input               its search field
 *   data-filter-value="tag"         its tabs (role="tab"); "all" or empty matches everything
 *   data-filter-item                one filtered element
 *   data-filter-text                words searched instead of the item's text
 *   data-filter-tags                tags of the item, separated by spaces
 *   data-filter-count               receives the number of visible items
 *   data-filter-empty               shown when nothing matches
 *   data-filter-query               receives the search text
 *   data-filter-clear               button that empties the search and selects "all"
 *   data-singular, data-plural      nouns appended to a count ("3 templates")
 *   data-zero                       text of a count of 0
 *   data-live-refresh="#id"         toggle: re-fetches the page and refreshes that element
 *   data-live-interval="30000"      its period in milliseconds
 *   data-password-toggle="#id"      toggle: shows or masks that input
 *   data-label, data-label-pressed  aria-label of a toggle when released and when pressed
 *   data-bind-source="key"          input mirrored into the [data-bind="key"] elements
 *   data-bind="key"                 element that shows the value
 *   data-bind-scope                 limits a binding to this element
 *   data-bind-empty                 text shown while the value is empty
 *   data-bind-format="json|shell"   escapes the value for a JSON/TOML string or a quoted shell word
 *   data-select-all="#id"           checkbox that checks every item of the target
 *   data-select-item                one selectable checkbox (or radio, to count a choice)
 *   data-select-count="#id"         receives the number of checked items
 *   data-select-bar="#id"           hidden while nothing is checked
 *   data-select-empty="#id"         hidden while something is checked
 *   data-check-all="#id"            button that checks the visible radios or checkboxes...
 *   data-check-value="value"        ...carrying this value in the target
 *   data-dirty                      form that tracks unsaved changes
 *   data-dirty-count                receives the number of changed fields
 *   data-dirty-submit               submit button disabled while nothing changed
 *   data-sidebar-toggle             toggle: collapses the desktop sidebar
 *   data-oauth-paste                box that finishes OAuth from a pasted callback address
 *   data-oauth-paste-submit         its button
 *   data-oauth-paste-error          its error text
 *   data-shortcut="mod+k"           field focused by Cmd+K (Apple) or Ctrl+K
 *   data-shortcut-hint              <kbd> that receives the platform's spelling of it
 *
 * State written for app.css
 *   html[data-dialog-open]          a modal dialog is open
 *   html[data-sidebar="collapsed"]  desktop sidebar collapsed
 *   data-dirty="true|false"         form has unsaved changes
 *   data-copied, data-copy-failed   copy feedback, for 2 s
 *   data-leaving                    toast fading out before removal
 *   data-popover-positioned         menu placed with --popover-top and --popover-left
 *   data-was-disabled               a control's own disabled state while its group is hidden
 *   hidden, disabled, required, title, tabindex, aria-busy, aria-current, aria-expanded,
 *   aria-selected, aria-pressed, aria-label, aria-invalid
 *
 * CSS custom properties set: --popover-top and --popover-left (px) on a menu popover, only in
 * browsers without CSS anchor positioning.
 * Events (bubbling): app:enhance, app:dialog-open, app:dialog-close, app:async-success,
 * app:async-error. Request headers: X-Requested-With, X-CSRF-Token, X-Fragment.
 */
'use strict'
;(() => {
  if (window.MyMCPs) return
  window.MyMCPs = { enhance, toast, confirm: confirmDialog }

  const GENERIC_ERROR = 'Something went wrong. Try again.'
  const COPY_ERROR = 'Copy failed. Select the text and copy it manually.'
  const REGION_HTML = '<div class="toast-region" data-toasts></div>'
  const TOAST_HTML = `<div class="toast" role="status"><p class="toast__message"></p><button
    type="button" class="toast__dismiss" data-toast-dismiss aria-label="Dismiss">\u00d7</button></div>`
  const CONFIRM_HTML = `<dialog class="dialog dialog--sm" role="alertdialog">
    <div class="dialog__header"><div class="dialog__heading">
      <h2 class="dialog__title" data-confirm-title-slot>Are you sure?</h2></div></div>
    <div class="dialog__body"><p data-confirm-message-slot></p></div>
    <div class="dialog__footer">
      <button type="button" class="button button--secondary" data-dialog-close>Cancel</button>
      <button type="button" class="button button--primary" data-confirm-accept>Confirm</button>
    </div></dialog>`
  const TYPED = /^(text|search|email|url|tel|number|password|textarea)$/ // autosubmit as you type
  const APPLE = /mac|iphone|ipad/i.test(navigator.userAgentData?.platform || navigator.platform)
  const ZONE = Intl.DateTimeFormat().resolvedOptions().timeZone
  const root = document.documentElement

  // ------------------------------------------------------------------------------- helpers
  const $ = (selector, scope = document) => scope.querySelector(selector)
  const $$ = (selector, scope = document) => [...scope.querySelectorAll(selector)]
  // Like $$, but the scope itself may match: fragments are enhanced from their own root.
  const within = (scope, css) => (scope.matches?.(css) ? [scope] : []).concat($$(css, scope))
  const byRef = (ref) => (ref ? document.getElementById(ref.replace(/^#/, '')) : null)
  const visible = (node) => node.checkVisibility?.() ?? true
  const emit = (target, name, detail) =>
    target.dispatchEvent(new CustomEvent(name, { bubbles: true, detail }))
  // Delegated listener on the document, for events whose target is an element.
  const on = (type, handler, capture = false) =>
    document.addEventListener(type, (e) => e.target instanceof Element && handler(e), capture)
  const replaceUrl = (url) => attempt(() => history.replaceState(history.state, '', url))

  // Storage and history throw when blocked or on file:// pages: both are best effort.
  function attempt(action) {
    try {
      return action()
    } catch {
      return null
    }
  }

  // Builds an element from a constant of this file, never from user text.
  function fromHtml(html) {
    const template = document.createElement('template')
    template.innerHTML = html
    return template.content.firstElementChild
  }

  // "3 templates", "1 template", the data-zero text, or the bare number without nouns.
  function setCount(target, count) {
    const { zero, singular, plural = singular } = target.dataset
    const noun = count === 1 ? singular : plural
    target.textContent = count === 0 && zero ? zero : noun ? `${count} ${noun}` : count
  }

  // Toggle buttons: aria-pressed, and aria-label taken from data-label / data-label-pressed.
  function setPressed(button, pressed) {
    const { dataset } = button
    if (dataset.labelPressed && !button.matches('[aria-pressed="true"]'))
      dataset.label ??= button.getAttribute('aria-label') ?? ''
    button.setAttribute('aria-pressed', pressed)
    const label = pressed ? dataset.labelPressed : dataset.label
    if (dataset.labelPressed && label !== undefined) button.setAttribute('aria-label', label)
  }

  // Next item for arrow-key navigation in a tab list or a menu, wrapping at both ends.
  function step(items, current, key, [previous, next]) {
    const index = items.indexOf(current)
    const count = items.length
    const moves = { Home: 0, End: count - 1, [next]: index + 1, [previous]: index - 1 + count }
    return key in moves ? items[moves[key] % count] : undefined
  }

  // First usable [autofocus], else first invalid field, else the fallback.
  function focusFirst(scope, fallback) {
    const usable = (node) => !node.disabled && visible(node)
    const fields = [...$$('[autofocus]', scope), ...$$('[aria-invalid="true"]', scope)]
    const target = fields.find(usable) || fallback
    target?.focus()
  }

  // Remembers the focused control of a region about to be replaced, to focus its twin afterwards.
  function focusKeeper(scope) {
    const active = document.activeElement
    if (active === document.body || !scope.contains(active)) return () => {}
    const { id, name, type, value } = active
    const box = /^(radio|checkbox)$/.test(type) ? `[value="${CSS.escape(value)}"]` : ''
    const selector = id ? `#${CSS.escape(id)}` : name && `[name="${CSS.escape(name)}"]${box}`
    return () => {
      const twin = selector && $(selector, scope)
      if (twin && !scope.contains(document.activeElement)) twin.focus()
    }
  }

  // ------------------------------------------------------------------- requests and fragments
  // Says the request comes from app.js and which element receives the HTML; carries the CSRF
  // token when it changes state.
  function request(url, { method = 'GET', body, target, signal } = {}) {
    const headers = { 'X-Requested-With': 'fetch', 'Accept': 'text/html' }
    const csrf = $('meta[name="csrf-token"]')?.content
    if (method !== 'GET' && csrf) headers['X-CSRF-Token'] = csrf
    if (target?.id) headers['X-Fragment'] = encodeURIComponent(target.id)
    return fetch(url, { method, body, headers, signal, credentials: 'same-origin' })
  }

  // Resolves to { html } for a fragment (200-299 or 422), {} for a 204, { failed } for anything
  // else, and { away } when the server asked for a full navigation: the X-Location header, or a
  // redirect that fetch already followed.
  async function fetchHtml(url, options) {
    try {
      const response = await request(url, options)
      const { status, headers } = response
      const next = headers.get('X-Location') || (response.redirected && response.url)
      if (next) {
        location.assign(next)
        return { away: true }
      }
      if (status === 204) return { status }
      const isHtml = headers.get('Content-Type')?.includes('text/html')
      if (isHtml && (response.ok || status === 422)) return { status, html: await response.text() }
    } catch {
      // Reported by the caller like an error status.
    }
    return { failed: true }
  }

  // Replaces the content of an element with markup rendered by our own server (trusted HTML),
  // given as a string or as nodes parsed from a full page.
  function swap(target, content, refocus = focusKeeper(target)) {
    const toasts = $('[data-toasts]')
    if (toasts && target.contains(toasts)) document.body.append(toasts)
    if (typeof content === 'string') target.innerHTML = content
    else target.replaceChildren(...content)
    enhance(target)
    refocus()
  }

  // ------------------------------------------------------------------------------- dialogs
  let openDialogs = [] // in opening order: the last one is on top
  let pressedBackdrop = false
  const openers = new WeakMap() // dialog -> the trigger that opened it
  const dialogLoads = new WeakMap() // dialog -> trigger of its latest content request
  const topModal = () => openDialogs.findLast((dialog) => dialog.matches(':modal'))
  // data-dialog-fetch="/url", or the href of the link when the attribute is empty.
  const dialogUrl = (t) => t.dataset.dialogFetch || (t.dataset.dialogFetch === '' && t.href)

  // The opener carries aria-current="true" while its dialog is open (CSS highlights its row).
  function markOpener(dialog, trigger) {
    const previous = openers.get(dialog)
    if (previous?.getAttribute('aria-current') === 'true') previous.removeAttribute('aria-current')
    openers.set(dialog, trigger)
    if (trigger?.getAttribute('aria-current') === null) trigger.setAttribute('aria-current', 'true')
  }

  // A refresh may have re-rendered the opener: its twin takes over the marker and the focus return.
  function reconnectOpener(dialog) {
    const lost = openers.get(dialog)
    if (!lost || lost.isConnected) return
    const url = dialogUrl(lost)
    const twin =
      byRef(lost.id) || (url && $$('[data-dialog-open]').find((t) => dialogUrl(t) === url))
    markOpener(dialog, twin)
  }

  function triggerDialog(trigger, event) {
    const dialog = byRef(trigger.dataset.dialogOpen)
    // A modified click on a link keeps opening it in a new tab or window.
    const modified = event.metaKey || event.ctrlKey || event.shiftKey
    if (!dialog || (modified && trigger.matches('a[href]'))) return
    event.preventDefault()
    if (dialogUrl(trigger)) loadDialog(dialog, trigger, dialogUrl(trigger))
    else openDialog(dialog, trigger)
  }

  function openDialog(dialog, trigger) {
    if (!dialog.isConnected) return
    const modal = dialog.dataset.dialogModal !== 'false'
    if (trigger && !dialog.contains(trigger)) markOpener(dialog, trigger)
    const opening = !dialog.open
    if (opening) {
      if (modal) dialog.showModal()
      else dialog.show()
      openDialogs.push(dialog)
      emit(dialog, 'app:dialog-open')
    }
    // Without a field to start on, the dialog takes the focus itself: it is read from its title,
    // and its close button does not light up when the server renders it open. A dialog whose
    // content was swapped under the person's hands keeps the focus where it is.
    const fallback = opening || !dialog.contains(document.activeElement) ? dialog : null
    if (fallback) dialog.tabIndex = -1
    focusFirst(dialog, fallback)
    syncDialogs()
  }

  async function loadDialog(dialog, trigger, url) {
    if (trigger.getAttribute('aria-busy') === 'true') return
    // A trigger inside the dialog swaps its content: the outside opener stays the opener.
    const opener = dialog.contains(trigger) ? null : trigger
    dialogLoads.set(dialog, trigger)
    trigger.setAttribute('aria-busy', 'true')
    const { html, away } = await fetchHtml(url, { target: dialog })
    trigger.removeAttribute('aria-busy')
    // Dropped when the page is leaving, or when a later trigger took over meanwhile.
    if (away || dialogLoads.get(dialog) !== trigger) return
    if (html === undefined) return toast(GENERIC_ERROR, { tone: 'error' })
    swap($('[data-fragment]', dialog) || dialog, html)
    openDialog(dialog, opener)
    if (trigger.hasAttribute('data-dialog-history')) replaceUrl(url)
  }

  function dialogClosed(dialog) {
    const opener = openers.get(dialog)
    const back = opener?.dataset.dialogReturn || dialog.dataset.dialogReturn
    markOpener(dialog, null)
    if (back) replaceUrl(back)
    if (dialog.hasAttribute('data-dialog-reset')) {
      for (const form of $$('form', dialog)) form.reset()
      $$('[data-password-toggle][aria-pressed="true"]', dialog).forEach(togglePassword)
    }
    syncDialogs()
    emit(dialog, 'app:dialog-close')
    // A trigger inside a closed menu cannot take focus: its menu button stands in.
    const menu = opener?.closest('[popover]:not(:popover-open)')
    const target = menu ? $(`[popovertarget="${CSS.escape(menu.id)}"]`) : opener
    if (target?.isConnected) target.focus()
  }

  // Keeps the scroll lock and the toast region in step with the open dialogs.
  function syncDialogs() {
    openDialogs = openDialogs.filter((dialog) => dialog.isConnected && dialog.open)
    root.toggleAttribute('data-dialog-open', Boolean(topModal()))
    placeToasts()
  }

  // A click lands on the dialog element itself when it hits the backdrop.
  function hitsBackdrop({ target, clientX: x, clientY: y }) {
    if (!target.matches('dialog:modal')) return false
    const box = target.getBoundingClientRect()
    return x < box.left || x > box.right || y < box.top || y > box.bottom
  }

  // ------------------------------------------------------------------- menus and popovers
  const invokers = new WeakMap() // popover -> the button that last opened it
  const anchored = CSS.supports('position-area: bottom')
  const MENUS = '[popover]:is(.menu, [role="menu"])'
  const MENU_ITEMS = '[role="menuitem"]:not(:disabled)'
  const GAP = 4 // between a menu and its button
  const EDGE = 8 // between a menu and the viewport

  // Fallback without CSS anchor positioning: under the invoker with right edges aligned (left
  // edges for .menu--start), above it when there is no room below, always inside the viewport.
  function placePopover(popover) {
    const invoker = invokers.get(popover) || $(`[popovertarget="${CSS.escape(popover.id)}"]`)
    if (!invoker?.isConnected || !popover.matches(':popover-open')) return
    const anchor = invoker.getBoundingClientRect()
    const { width, height } = popover.getBoundingClientRect()
    const below = anchor.bottom + GAP
    const above = anchor.top - GAP - height
    const top = below + height > innerHeight - EDGE && above >= EDGE ? above : below
    const aligned = popover.matches('.menu--start') ? anchor.left : anchor.right - width
    const left = Math.max(EDGE, Math.min(aligned, innerWidth - EDGE - width))
    popover.style.setProperty('--popover-top', `${Math.round(top)}px`)
    popover.style.setProperty('--popover-left', `${Math.round(left)}px`)
    popover.setAttribute('data-popover-positioned', '')
  }

  function popoverToggled({ target: popover, newState }) {
    const buttons = $$(`[popovertarget="${CSS.escape(popover.id)}"]`)
    for (const button of buttons.filter((b) => b.getAttribute('popovertargetaction') !== 'hide'))
      button.setAttribute('aria-expanded', newState === 'open')
    if (newState === 'open') $(MENU_ITEMS, popover)?.focus()
  }

  // ---------------------------------------------------------------- tabs and disclosure
  function selectTab(tab) {
    for (const other of $$('[role="tab"]', tab.closest('[role="tablist"]'))) {
      const selected = other === tab
      other.setAttribute('aria-selected', selected)
      if (selected) other.removeAttribute('tabindex')
      else other.setAttribute('tabindex', '-1')
      const panel = byRef(other.getAttribute('aria-controls'))
      if (panel) panel.hidden = !selected
    }
    const panelId = tab.getAttribute('aria-controls')
    if (panelId && tab.closest('[data-tabs]')?.dataset.tabs === 'hash') replaceUrl(`#${panelId}`)
    const filter = tab.closest('[data-filter]')
    if (filter) applyFilter(filter)
  }

  function restoreHashTab(tabs) {
    const id = decodeURIComponent(location.hash.slice(1))
    const tab = id && $$('[role="tab"]', tabs).find((t) => t.getAttribute('aria-controls') === id)
    if (tab && tab.getAttribute('aria-selected') !== 'true') selectTab(tab)
  }

  function toggleDisclosure(button) {
    const hidden = byRef(button.dataset.toggle)?.toggleAttribute('hidden')
    if (hidden !== undefined) button.setAttribute('aria-expanded', !hidden)
  }

  // ------------------------------------------------------------------------------ clipboard
  async function copy(trigger) {
    const source = byRef(trigger.dataset.copyTarget)
    const field = source?.matches('input, textarea, select')
    const text = trigger.dataset.copy || (field ? source.value : source?.textContent.trim()) || ''
    const written = attempt(() => navigator.clipboard.writeText(text))
    const copied = await written?.then(() => true).catch(() => false)
    if (!copied) toast(COPY_ERROR, { tone: 'error' })
    // The feedback of a previous click is still showing.
    if (trigger.matches('[data-copied], [data-copy-failed]')) return
    const label = $('[data-copy-label]', trigger)
    const before = { text: label?.textContent, name: trigger.getAttribute('aria-label') }
    const state = copied ? 'data-copied' : 'data-copy-failed'
    trigger.setAttribute(state, '')
    if (copied && before.name !== null) trigger.setAttribute('aria-label', 'Copied')
    if (copied && label) label.textContent = label.dataset.copiedLabel || 'Copied'
    setTimeout(() => {
      trigger.removeAttribute(state)
      if (before.name !== null) trigger.setAttribute('aria-label', before.name)
      if (label) label.textContent = before.text
    }, 2000)
  }

  // -------------------------------------------------------------------------------- toasts
  const armedToasts = new WeakSet()
  const toastRegion = () => $('[data-toasts]') || document.body.appendChild(fromHtml(REGION_HTML))

  // A modal dialog makes the page inert and sits above it, so the region follows the topmost
  // one and goes back to <body> when it closes.
  function placeToasts() {
    const region = $('[data-toasts]')
    const host = topModal() || (region?.closest('dialog') ? document.body : region?.parentElement)
    if (region && region.parentElement !== host) host.append(region)
  }

  function adoptToast(node) {
    if (node.parentElement !== toastRegion()) toastRegion().append(node)
    if (armedToasts.has(node)) return
    armedToasts.add(node)
    if (!node.matches('.toast--error, [data-toast-sticky]')) holdToast(node, 5000)
  }

  // Hover and focus hold an info toast: it is looked at again every second instead of removed.
  function holdToast(node, delay) {
    const held = () => node.matches(':hover, :focus-within')
    setTimeout(() => (held() ? holdToast(node, 1000) : dismissToast(node)), delay)
  }

  // CSS fades the toast out; the timeout covers a missing or reduced transition.
  function dismissToast(node) {
    if (!node?.isConnected || node.hasAttribute('data-leaving')) return
    node.setAttribute('data-leaving', '')
    node.addEventListener('transitionend', (event) => event.target === node && node.remove())
    setTimeout(() => node.remove(), 300)
  }

  function toast(message, { tone = 'info', sticky = false } = {}) {
    const error = tone === 'error'
    const template = byRef(error ? 'toast-error-template' : 'toast-template')
    const node = template?.content.firstElementChild.cloneNode(true) ?? fromHtml(TOAST_HTML)
    if (error && !template) {
      node.classList.add('toast--error')
      node.setAttribute('role', 'alert')
    }
    node.setAttribute('data-toast', '')
    if (sticky) node.setAttribute('data-toast-sticky', '')
    $('.toast__message', node).textContent = message
    adoptToast(node)
    placeToasts()
    return node
  }

  // --------------------------------------------------------------------------- async forms
  const pending = new WeakMap() // form -> { controller, release } while its request runs
  const autosubmitTimers = new WeakMap()

  function cancelPending(form) {
    const state = pending.get(form)
    if (!state) return
    pending.delete(form)
    state.controller.abort()
    state.release()
  }

  // Reads the form as a native submit would: URL, method, and a body with the same encoding.
  function serialize(form, submitter) {
    const setting = (own, inherited) =>
      submitter?.getAttribute(own) || form.getAttribute(inherited) || ''
    const method = setting('formmethod', 'method').toUpperCase() || 'GET'
    const url = new URL(setting('formaction', 'action') || location.href, document.baseURI)
    const data = new FormData(form, submitter)
    const text = ([name, value]) => [name, typeof value === 'string' ? value : value.name]
    const fields = () => new URLSearchParams([...data].map(text))
    if (method === 'GET') url.search = fields()
    const multipart = setting('formenctype', 'enctype') === 'multipart/form-data'
    return { method, url, body: method === 'GET' ? undefined : multipart ? data : fields() }
  }

  async function submitAsync(form, submitter, target) {
    if (pending.has(form)) return
    const { method, url, body } = serialize(form, submitter)
    const controller = new AbortController()
    const focused = document.activeElement
    const refocus = focusKeeper(target)
    const busy = [form, submitter].filter(Boolean)
    const buttons = [...form.elements].filter((c) => c.type === 'submit' && !c.disabled)
    const release = () => {
      busy.forEach((node) => node.removeAttribute('aria-busy'))
      buttons.forEach((button) => (button.disabled = false))
      // Disabling the submitter dropped the focus it had.
      if (focused?.isConnected && document.activeElement === document.body) focused.focus()
    }
    pending.set(form, { controller, release })
    busy.forEach((node) => node.setAttribute('aria-busy', 'true'))
    buttons.forEach((button) => (button.disabled = true))
    const options = { method, body, target, signal: controller.signal }
    const { html, status, failed, away } = await fetchHtml(url, options)
    // The pending state stays while the page is leaving; a newer submit owns it otherwise.
    if (away || pending.get(form)?.controller !== controller) return
    cancelPending(form)
    if (failed) {
      toast(GENERIC_ERROR, { tone: 'error' })
      return emit(target, 'app:async-error', { form })
    }
    if (html !== undefined) {
      swap(target, html, refocus)
      focusFirst($('dialog:modal', target) || target)
    }
    if (method === 'GET' && form.hasAttribute('data-async-history')) replaceUrl(url)
    if (form.isConnected && form.hasAttribute('data-dirty')) syncDirty(form, true)
    emit(target, 'app:async-success', { form, status })
  }

  // ---------------------------------------------------------------------------- plain forms
  const held = new WeakMap() // plain form -> what lets it go, while its answer is awaited
  const DOWNLOAD_SETTLE = 1000 // ms a download form stays busy: about when the file starts

  // A plain post that takes a while says so on its submit button ([data-busy-label]) until its
  // answer replaces the page. The answer to a [data-download] form is a file, which leaves the
  // page where it is: that form lets go by itself, resets, and closes its dialog. The browser
  // reads the fields after the submit event, so nothing is disabled or emptied before a timer.
  function holdPlain(form, submitter) {
    const button = submitter || [...form.elements].find((control) => control.type === 'submit')
    const label = button?.dataset.busyLabel
    const download = form.hasAttribute('data-download')
    if (held.has(form) || (!label && !download)) return
    const busy = [form, button].filter(Boolean)
    const content = button && [...button.childNodes]
    const timers = [
      setTimeout(() => {
        busy.forEach((node) => node.setAttribute('aria-busy', 'true'))
        if (label) button.replaceChildren(label)
        if (button) button.disabled = true
      }),
    ]
    const release = () => {
      timers.forEach(clearTimeout)
      held.delete(form)
      busy.forEach((node) => node.removeAttribute('aria-busy'))
      if (label) button.replaceChildren(...content)
      if (button) button.disabled = false
    }
    held.set(form, release)
    if (!download) return
    timers.push(
      setTimeout(() => {
        release()
        form.reset()
        form.closest('dialog[open]')?.close()
      }, DOWNLOAD_SETTLE)
    )
  }

  // A change replaces the request in flight, so the last choice always wins.
  function autosubmit(form) {
    if (!form.isConnected) return
    cancelPending(form)
    form.requestSubmit()
  }

  // ------------------------------------------------------------------------ confirm prompts
  const confirmed = new WeakSet() // forms whose held submit was accepted

  function confirmDialog(message, { title, label, tone, trigger } = {}) {
    const template = byRef('confirm-template')
    const dialog = template?.content.firstElementChild.cloneNode(true) ?? fromHtml(CONFIRM_HTML)
    const accept = $('[data-confirm-accept]', dialog)
    // One prompt at a time (it is modal and removed on close), so fixed ids are enough.
    const fill = (name, relation, text) => {
      const slot = $(`[data-confirm-${name}-slot]`, dialog)
      if (text) slot.textContent = text
      slot.id = `confirm-${name}`
      dialog.setAttribute(relation, slot.id)
    }
    fill('title', 'aria-labelledby', title)
    fill('message', 'aria-describedby', message)
    if (label) accept.textContent = label
    if (tone === 'critical') accept.classList.replace('button--primary', 'button--critical')
    // Cancel is focused first, so that Enter never confirms by accident.
    const cancel = $('[data-dialog-close]', accept.parentElement)
    if (!$('[autofocus]', dialog)) cancel?.setAttribute('autofocus', '')
    document.body.append(dialog)
    return new Promise((resolve) => {
      dialog.addEventListener('close', () => {
        resolve(dialog.returnValue === 'accept')
        dialog.remove()
      })
      openDialog(dialog, trigger)
    })
  }

  // Holds a submit that needs a confirmation and replays it once accepted.
  function heldForConfirm(event) {
    const { target: form, submitter } = event
    const source = submitter?.hasAttribute('data-confirm') ? submitter : form
    if (!source.hasAttribute('data-confirm') || confirmed.has(form)) return false
    event.preventDefault()
    const count = [...form.elements].filter((c) => c.matches('[data-select-item]:checked')).length
    const fill = (text) => text?.replaceAll('{count}', count)
    const { confirm, confirmTitle: title, confirmLabel: label, confirmTone: tone } = source.dataset
    const options = { title: fill(title), label, tone, trigger: submitter }
    confirmDialog(fill(confirm), options).then((accepted) => {
      if (!accepted) return
      confirmed.add(form)
      attempt(() => form.requestSubmit(submitter))
      confirmed.delete(form)
    })
    return true
  }

  // ------------------------------------------------------------------- dynamic field groups
  // Current values of the control(s) called `name`: "on"/"off" for a lone checkbox, the checked
  // values of a group, or the trimmed text.
  function fieldValues(form, name) {
    const controls = [...(form ? form.elements : $$('input, select, textarea'))].filter(
      (control) => control.name === name && (form || !control.form)
    )
    const boxes = controls.filter((control) => control.type === 'checkbox')
    if (boxes.length === 1) return [boxes[0].checked ? 'on' : 'off']
    const group = boxes.length ? boxes : controls.filter((control) => control.type === 'radio')
    if (group.length) return group.filter((control) => control.checked).map((c) => c.value)
    return controls.slice(0, 1).map((control) => String(control.value ?? '').trim())
  }

  // "name=a|b", "name!=a", joined by "&". "name:origin=https://host" compares the origin of a
  // URL field and holds while the URL is incomplete: a URL being typed is not a decision yet.
  function conditionHolds(form, expression) {
    return expression.split('&').every((condition) => {
      const [, key = '', negated, wanted = ''] = condition.match(/^(.*?)(!?)=(.*)$/) ?? []
      const [name, modifier] = key.trim().split(':')
      const values = fieldValues(form, name)
      const options = wanted.split('|')
      const origin = (value) => URL.parse(value)?.origin
      const sameOrigin = (value) => !origin(value) || options.includes(origin(value))
      const holds = modifier ? values.every(sameOrigin) : values.some((v) => options.includes(v))
      return negated ? !holds : holds
    })
  }

  // Hidden groups are disabled as well, so that they are neither validated nor submitted;
  // data-was-disabled keeps the state each control had before. A control stays out of the form
  // while any group around it is hidden, not only the closest one.
  function syncConditions(form) {
    const mine = (css) => $$(css, form ?? document).filter((node) => node.closest('form') === form)
    const groups = mine('[data-show-when], [data-hide-when]')
    for (const node of groups) {
      const { showWhen, hideWhen } = node.dataset
      const unless = showWhen === undefined
      node.hidden = conditionHolds(form, unless ? hideWhen : showWhen) === unless
    }
    const concealed = (control) =>
      Boolean(control.closest('[data-show-when][hidden], [data-hide-when][hidden]'))
    for (const node of groups) {
      for (const control of within(node, 'input, select, textarea, button, fieldset')) {
        const hidden = concealed(control)
        if (hidden) control.dataset.wasDisabled ??= control.disabled
        else if (!control.dataset.wasDisabled) continue
        control.disabled = hidden || control.dataset.wasDisabled === 'true'
        if (!hidden) delete control.dataset.wasDisabled
      }
    }
    for (const control of mine('[data-optional-when]'))
      control.required = !conditionHolds(form, control.dataset.optionalWhen)
  }

  function syncForm(form) {
    syncConditions(form)
    if (form?.hasAttribute('data-dirty')) syncDirty(form)
  }

  // ------------------------------------------------------------------------ repeatable rows
  // Parts of a repeat group, leaving out those of a group nested in one of its rows.
  const repeatParts = (repeat, css) =>
    $$(css, repeat).filter((node) => node.closest('[data-repeat]') === repeat)

  function syncRepeat(repeat) {
    const count = repeatParts(repeat, '[data-repeat-row]').length
    const { repeatMin = 0, repeatMax = Infinity } = repeat.dataset
    for (const add of repeatParts(repeat, '[data-repeat-add]')) add.hidden = count >= repeatMax
    for (const remove of repeatParts(repeat, '[data-repeat-remove]'))
      remove.disabled = count <= repeatMin
  }

  function addRepeatRow(button) {
    const repeat = button.closest('[data-repeat]')
    const [template] = repeatParts(repeat, 'template[data-repeat-template]')
    const [list] = repeatParts(repeat, '[data-repeat-list]')
    if (!template || !list) return
    // The next free index is one past the highest in use: indexes are never reused.
    const sample = $('[name*="__i__"]', template.content)?.getAttribute('name') ?? '__i__'
    const escaped = sample.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
    const pattern = new RegExp(`^${escaped.replace('__i__', '(\\d+)')}$`)
    const used = $$('[name]', list).map((c) => c.getAttribute('name').match(pattern)?.[1])
    const index = Math.max(-1, ...used.filter(Boolean).map(Number)) + 1
    const row = template.content.cloneNode(true)
    for (const node of $$('*', row))
      for (const attribute of [...node.attributes])
        attribute.value = attribute.value.replaceAll('__i__', index)
    const added = [...row.children]
    list.append(row)
    added.forEach((node) => enhance(node))
    syncRepeat(repeat)
    syncForm(button.form)
    $('input, select, textarea', added[0])?.focus()
  }

  function removeRepeatRow(button) {
    const repeat = button.closest('[data-repeat]')
    const form = button.form
    const rows = repeatParts(repeat, '[data-repeat-row]')
    const index = rows.indexOf(button.closest('[data-repeat-row]'))
    if (index < 0 || rows.length <= Number(repeat.dataset.repeatMin || 0)) return
    rows[index].remove()
    syncRepeat(repeat)
    syncForm(form)
    const neighbour = rows[index + 1] || rows[index - 1]
    const next = [neighbour && $('[data-repeat-remove]', neighbour), $('[data-repeat-add]', repeat)]
    next.find((node) => node && !node.disabled && !node.hidden)?.focus()
  }

  // ---------------------------------------------------------------------------- local time
  // Same formats as the React app: 07/10/2026 and 07/10/2026, 18:05:09.
  const DAY_FIRST = { day: '2-digit', month: '2-digit', year: 'numeric' }
  const TIME = { hour: '2-digit', minute: '2-digit', second: '2-digit' }
  const dateFormat = new Intl.DateTimeFormat('en-GB', DAY_FIRST)
  const dateTimeFormat = new Intl.DateTimeFormat('en-GB', { ...DAY_FIRST, ...TIME })
  // Tables show dates to the minute; the full date-time is in the title.
  const minuteFormat = new Intl.DateTimeFormat('en-GB', { ...DAY_FIRST, hour: '2-digit', minute: '2-digit' })
  let timeZoneSubmitted = false

  function relativeTime(date) {
    const elapsed = Date.now() - date.getTime()
    const minutes = Math.floor(Math.abs(elapsed) / 60000)
    const hours = Math.floor(minutes / 60)
    const days = Math.floor(hours / 24)
    if (minutes < 1) return 'just now'
    if (days >= 7) return dateFormat.format(date)
    if (days === 1) return elapsed > 0 ? 'yesterday' : 'tomorrow'
    const amount = days ? `${days} days` : hours ? `${hours} h` : `${minutes} min`
    return elapsed > 0 ? `${amount} ago` : `in ${amount}`
  }

  // The server-rendered text (UTC) stays when the value is not a full date-time.
  function renderTime(time) {
    const value = time.getAttribute('datetime')
    const date = new Date(value)
    if (!value.includes('T') || Number.isNaN(date.getTime())) return
    const format = time.dataset.format || 'datetime'
    const formats = {
      relative: relativeTime(date),
      date: dateFormat.format(date),
      minute: minuteFormat.format(date),
    }
    time.textContent = formats[format] ?? dateTimeFormat.format(date)
    if (format in formats) time.title = dateTimeFormat.format(date)
  }

  function timeZoneLabel() {
    const options = { timeZone: ZONE, timeZoneName: 'longOffset' }
    const parts = new Intl.DateTimeFormat('en', options).formatToParts(new Date())
    const offset = parts.find((part) => part.type === 'timeZoneName')?.value.replace('GMT', 'UTC')
    return offset ? `${ZONE} (${offset})` : ZONE
  }

  // Tells the server the viewer's time zone, and reloads the data once when it had another one.
  function syncTimeZone(input) {
    if (!ZONE || input.value === ZONE) return
    input.value = ZONE
    const sent = new URLSearchParams(location.search).get(input.name) === ZONE
    if (timeZoneSubmitted || sent || !input.form?.hasAttribute('data-timezone-sync')) return
    timeZoneSubmitted = true
    input.form.requestSubmit()
  }

  // datetime-local inputs carry no zone: the instant in data-utc is shown in local time here and
  // converted back to a UTC instant when the form data is built.
  function localizeDateTime(input) {
    const date = new Date(input.dataset.utc)
    if (!input.dataset.utc || Number.isNaN(date.getTime())) return
    const pad = (part) => String(part).padStart(2, '0')
    const day = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
    input.defaultValue = `${day}T${pad(date.getHours())}:${pad(date.getMinutes())}`
  }

  function sendUtc({ target: form, formData }) {
    for (const input of $$('input[data-utc]', form)) {
      const date = new Date(input.value)
      if (formData.has(input.name) && !Number.isNaN(date.getTime()))
        formData.set(input.name, date.toISOString())
    }
  }

  // --------------------------------------------------------------------- client-side filter
  // Lower case without diacritics, for a forgiving search.
  const fold = (text) => text.normalize('NFD').replace(/\p{Diacritic}/gu, '')

  // Shows the items that contain every word of the search and carry the selected tag.
  function applyFilter(filter) {
    const query = $('[data-filter-input]', filter)?.value.trim() ?? ''
    const words = fold(query.toLowerCase()).split(/\s+/).filter(Boolean)
    const tag = $('[data-filter-value][aria-selected="true"]', filter)?.dataset.filterValue || 'all'
    let count = 0
    for (const item of $$('[data-filter-item]', filter)) {
      const text = fold((item.dataset.filterText ?? item.textContent).toLowerCase())
      const tagged = tag === 'all' || (item.dataset.filterTags ?? '').split(/\s+/).includes(tag)
      item.hidden = !(tagged && words.every((word) => text.includes(word)))
      if (!item.hidden) count += 1
    }
    for (const node of $$('[data-filter-count]', filter)) setCount(node, count)
    for (const node of $$('[data-filter-empty]', filter)) node.hidden = count > 0
    for (const node of $$('[data-filter-query]', filter)) node.textContent = query
  }

  // A search looks in every category, as the template gallery always did; clearing resets both.
  function searchFilter(filter, clear = false) {
    const input = $('[data-filter-input]', filter)
    const all = $('[data-filter-value="all"], [data-filter-value=""]', filter)
    if (clear && input) input.value = ''
    if (all && (clear || input?.value.trim())) selectTab(all)
    else applyFilter(filter)
    if (clear) input?.focus()
  }

  // -------------------------------------------------------------------------- live refresh
  const liveTimers = new WeakMap() // pressed button -> interval id
  const liveLoads = new WeakSet() // buttons with a refresh in flight
  const liveKey = () => `mm-live:${location.pathname}`

  // The pressed state is remembered per page for the session when the person chose it.
  function setLive(button, on, chosen = false) {
    const interval = Number(button.dataset.liveInterval) || 30000
    if (chosen) attempt(() => sessionStorage[on ? 'setItem' : 'removeItem'](liveKey(), '1'))
    setPressed(button, on)
    button.title = `${on ? 'Disable' : 'Enable'} automatic refresh every ${interval / 1000} seconds`
    clearInterval(liveTimers.get(button))
    liveTimers.delete(button)
    const timer = on && setInterval(() => refreshLive(button), interval)
    if (timer) liveTimers.set(button, timer)
  }

  function restoreLive(button) {
    const on = attempt(() => sessionStorage.getItem(liveKey())) === '1'
    if (!button.title || on !== liveTimers.has(button)) setLive(button, on)
  }

  async function refreshLive(button) {
    if (!button.isConnected) return clearInterval(liveTimers.get(button))
    const target = byRef(button.dataset.liveRefresh)
    // Never behind a modal dialog, nor under an open panel or menu the new content would close.
    const covered = () => topModal() || $('dialog[open], :popover-open', target)
    const blocked = () => !target?.isConnected || document.hidden || covered()
    if (blocked() || liveLoads.has(button)) return
    liveLoads.add(button)
    const { html = '' } = await fetchHtml(location.href, { target })
    liveLoads.delete(button)
    const fresh = new DOMParser().parseFromString(html, 'text/html').getElementById(target.id)
    if (fresh && !blocked()) swap(target, [...fresh.childNodes])
  }

  // ---------------------------------------------- password, bound text, selection, dirty state
  function togglePassword(button) {
    const input = byRef(button.dataset.passwordToggle)
    if (!input) return
    input.type = input.type === 'password' ? 'text' : 'password'
    setPressed(button, input.type === 'text')
  }

  const bindFormats = {
    json: (value) => JSON.stringify(value).slice(1, -1),
    shell: (value) => value.replaceAll("'", `'"'"'`),
  }

  function syncBinding(source) {
    const value = source.value.trim()
    const scope = source.closest('[data-bind-scope]') || document
    for (const node of $$(`[data-bind="${CSS.escape(source.dataset.bindSource)}"]`, scope)) {
      const format = bindFormats[node.dataset.bindFormat] ?? String
      node.textContent = value ? format(value) : (node.dataset.bindEmpty ?? '')
    }
  }

  const selectable = (ref) => $$('input[data-select-item]:not(:disabled)', byRef(ref) ?? root)
  const selected = (ref) => selectable(ref).filter((item) => item.checked).length

  function syncSelection() {
    for (const all of $$('[data-select-all]')) {
      const total = selectable(all.dataset.selectAll).length
      const count = selected(all.dataset.selectAll)
      all.checked = total > 0 && count === total
      all.indeterminate = count > 0 && count < total
    }
    for (const node of $$('[data-select-count]')) setCount(node, selected(node.dataset.selectCount))
    for (const node of $$('[data-select-bar]')) node.hidden = !selected(node.dataset.selectBar)
    for (const node of $$('[data-select-empty]'))
      node.hidden = selected(node.dataset.selectEmpty) > 0
  }

  // Checks the radios or checkboxes that carry a value, skipping those a filter hides: a choice
  // made on a row the search hides is kept.
  function checkAll(button) {
    const inputs = $$(
      'input:is([type="radio"], [type="checkbox"])',
      byRef(button.dataset.checkAll) ?? root
    )
    const wanted = (input) => input.value === button.dataset.checkValue && !input.disabled
    const changed = inputs.filter((input) => wanted(input) && !input.closest('[hidden]'))
    for (const input of changed) input.checked = true
    changed.at(-1)?.dispatchEvent(new Event('change', { bubbles: true }))
  }

  const cleanState = new WeakMap() // form[data-dirty] -> its fields as rendered or last saved

  function formState(form) {
    const state = {}
    for (const [name, value] of new FormData(form))
      (state[name] ??= []).push(typeof value === 'string' ? value : `${value.name}:${value.size}`)
    return state
  }

  // Counts the field names whose values differ from the clean state.
  function syncDirty(form, saved = false) {
    const now = formState(form)
    if (saved || !cleanState.has(form)) cleanState.set(form, now)
    const clean = cleanState.get(form)
    const names = [...new Set([...Object.keys(clean), ...Object.keys(now)])]
    const changed = (name) => JSON.stringify(clean[name]) !== JSON.stringify(now[name])
    const count = names.filter(changed).length
    form.dataset.dirty = count > 0
    for (const node of $$('[data-dirty-count]', form)) setCount(node, count)
    // A form re-rendered with errors holds unsaved input even when nothing changed since.
    const idle = count === 0 && !$('[aria-invalid="true"]', form)
    for (const control of form.elements)
      if (control.matches('[data-dirty-submit]')) control.disabled = idle
  }

  // --------------------------------------------------------- sidebar, OAuth paste, shortcuts
  // The cookie lets the server render the collapsed rail directly, without a flash.
  function setSidebar(collapsed, chosen = false) {
    const age = collapsed ? 31536000 : 0
    if (chosen) document.cookie = `mm_sidebar=collapsed; path=/; SameSite=Lax; max-age=${age}`
    if (chosen) attempt(() => localStorage.setItem('mm-sidebar', collapsed ? 'collapsed' : 'open'))
    if (collapsed) root.dataset.sidebar = 'collapsed'
    else delete root.dataset.sidebar
    for (const button of $$('[data-sidebar-toggle]')) setPressed(button, collapsed)
  }

  // Maps the loopback address a provider redirected to onto this instance's OAuth callback;
  // called without `finish` it only clears the error.
  function oauthPaste(box, finish = true) {
    const input = $('input', box)
    const url = URL.parse(input.value.trim())
    const answer = url?.searchParams
    const valid = answer?.has('error') || (answer?.has('code') && answer.has('state'))
    if (finish && valid) return location.assign(`/mcps/oauth/callback${url.search}`)
    if (finish) input.setAttribute('aria-invalid', 'true')
    else input.removeAttribute('aria-invalid')
    for (const node of $$('[data-oauth-paste-error]', box)) node.hidden = !finish
  }

  // The first visible, enabled field with this shortcut; inside the modal dialog when one is open.
  function shortcutField(key) {
    const modal = topModal()
    const reachable = (field) =>
      !field.disabled && visible(field) && (modal?.contains(field) ?? true)
    return $$(`[data-shortcut="mod+${CSS.escape(key)}" i]`).find(reachable)
  }

  function labelShortcut(hint) {
    let scope = hint.parentElement
    while (scope && !$('[data-shortcut]', scope)) scope = scope.parentElement
    const letter = scope && $('[data-shortcut]', scope).dataset.shortcut.slice(-1).toUpperCase()
    if (letter) hint.textContent = APPLE ? `\u2318${letter}` : `Ctrl ${letter}`
  }

  // ------------------------------------------------------------------------------- enhance
  // Applies every behaviour that needs a look at the markup. Idempotent; run on the document
  // at load and on each fragment the server sends afterwards.
  function enhance(scope = document) {
    const each = (css, action) => within(scope, css).forEach((node) => action(node))
    each('[data-toast]', adoptToast)
    each('time[datetime]', renderTime)
    each('[data-timezone-label]', (node) => (node.textContent = timeZoneLabel()))
    each('input[data-utc]', localizeDateTime)
    each('[data-shortcut-hint]', labelShortcut)
    each('[popovertarget]:not([popovertargetaction="hide"], [aria-expanded])', (button) =>
      button.setAttribute('aria-expanded', 'false')
    )
    each('[data-tabs="hash"]', restoreHashTab)
    each('[data-repeat]', syncRepeat)
    each('[data-filter]', applyFilter)
    each('[data-bind-source]', syncBinding)
    each('[data-live-refresh]', restoreLive)
    // A stored choice wins over the rendered one and renews the cookie the server reads.
    const stored = attempt(() => localStorage.getItem('mm-sidebar'))
    if (within(scope, '[data-sidebar-toggle]').length)
      setSidebar((stored ?? root.dataset.sidebar) === 'collapsed', Boolean(stored))
    const conditional = within(scope, '[data-show-when], [data-hide-when], [data-optional-when]')
    new Set(conditional.map((node) => node.closest('form'))).forEach((form) => syncConditions(form))
    each('form[data-dirty]', syncDirty)
    syncSelection()
    openDialogs.forEach(reconnectOpener)
    each('dialog[data-open]', (dialog) => {
      dialog.removeAttribute('data-open')
      openDialog(dialog, byRef(dialog.dataset.dialogTrigger))
    })
    syncDialogs()
    each('input[data-timezone]', syncTimeZone)
    emit(scope, 'app:enhance')
  }

  // -------------------------------------------------------------------------------- events
  // Every action whose selector matches the click target or one of its ancestors runs, in order.
  const clickActions = {
    '[popover]:popover-open [role="menuitem"]': (item) => item.closest('[popover]').hidePopover(),
    '[data-dialog-open]': triggerDialog,
    '[data-dialog-close]': (button) => button.closest('dialog')?.close(),
    '[data-confirm-accept]': (button) => button.closest('dialog')?.close('accept'),
    '[role="tablist"] [role="tab"]': selectTab,
    '[data-toggle]': toggleDisclosure,
    '[data-reveal]': (trigger) => byRef(trigger.dataset.reveal)?.removeAttribute('hidden'),
    '[data-copy], [data-copy-target]': copy,
    '[data-toast-dismiss]': (button) => dismissToast(button.closest('[data-toast]')),
    '[data-repeat-add]': addRepeatRow,
    '[data-repeat-remove]': removeRepeatRow,
    '[data-filter-clear]': (button) => searchFilter(button.closest('[data-filter]'), true),
    '[data-live-refresh]': (button) => setLive(button, !liveTimers.has(button), true),
    '[data-password-toggle]': togglePassword,
    '[data-check-all]': checkAll,
    '[data-sidebar-toggle]': () => setSidebar(root.dataset.sidebar !== 'collapsed', true),
    '[data-oauth-paste-submit]': (button) => oauthPaste(button.closest('[data-oauth-paste]')),
  }

  // Releasing a text selection over the backdrop must not close the dialog: the press counts too.
  on('pointerdown', (event) => (pressedBackdrop = hitsBackdrop(event)))

  on('click', (event) => {
    const target = event.target
    if (pressedBackdrop && hitsBackdrop(event) && !target.hasAttribute('data-dialog-static'))
      target.close()
    const invoker = target.closest('[popovertarget]')
    if (invoker?.popoverTargetElement) invokers.set(invoker.popoverTargetElement, invoker)
    for (const [selector, action] of Object.entries(clickActions)) {
      const node = target.closest(selector)
      if (node && node.getAttribute('aria-disabled') !== 'true') action(node, event)
    }
  })

  on('keydown', (event) => {
    const { key, target } = event
    const tab = target.closest('[role="tablist"] [role="tab"]')
    const tabs = tab && $$('[role="tab"]', tab.closest('[role="tablist"]'))
    const item = target.closest('[role="menu"] [role="menuitem"]')
    const items = item && $$(MENU_ITEMS, item.closest('[role="menu"]'))
    const next = tab
      ? step(tabs, tab, key, ['ArrowLeft', 'ArrowRight'])
      : item && step(items, item, key, ['ArrowUp', 'ArrowDown'])
    const mod = (APPLE ? event.metaKey : event.ctrlKey) && !event.altKey && !event.shiftKey
    const field = mod && shortcutField(key.toLowerCase())
    if (event.isComposing) return
    if (key === 'Escape') {
      // Modal dialogs and popovers close natively; a side panel needs this.
      if (!event.defaultPrevented && !topModal() && !$(':popover-open')) openDialogs.at(-1)?.close()
    } else if (next) {
      event.preventDefault()
      if (tab) selectTab(next)
      next.focus()
    } else if (key === 'Enter' && target.matches('[data-oauth-paste] input, [data-filter-input]')) {
      // These fields sit inside forms that Enter must not submit.
      event.preventDefault()
      if (target.closest('[data-oauth-paste]')) oauthPaste(target.closest('[data-oauth-paste]'))
    } else if (field) {
      event.preventDefault()
      field.focus()
      field.select?.()
    }
  })

  on('input', ({ target }) => {
    const form = target.form ?? null
    syncForm(form)
    if (target.matches('[data-filter-input]')) searchFilter(target.closest('[data-filter]'))
    if (target.matches('[data-bind-source]')) syncBinding(target)
    if (target.matches('[data-oauth-paste] input'))
      oauthPaste(target.closest('[data-oauth-paste]'), false)
    if (!form?.hasAttribute('data-autosubmit') || !target.name || !TYPED.test(target.type)) return
    // Typing invalidates the request in flight at once; the new one waits for a pause.
    cancelPending(form)
    clearTimeout(autosubmitTimers.get(form))
    const timer = setTimeout(() => autosubmit(form), 300)
    autosubmitTimers.set(form, timer)
  })

  on('change', ({ target }) => {
    const form = target.form ?? null
    if (target.matches('[data-select-all]'))
      for (const item of selectable(target.dataset.selectAll)) item.checked = target.checked
    syncForm(form)
    syncSelection()
    const typed = TYPED.test(target.type)
    if (form?.hasAttribute('data-autosubmit') && target.name && !typed) autosubmit(form)
  })

  on('submit', (event) => {
    const { target: form, submitter } = event
    clearTimeout(autosubmitTimers.get(form))
    if (event.defaultPrevented || heldForConfirm(event)) return
    const method = submitter?.getAttribute('formmethod') || form.getAttribute('method') || ''
    const ref = submitter?.dataset.asyncTarget || form.dataset.asyncTarget
    const target = byRef(ref) || form.closest('[data-fragment]')
    if (!form.hasAttribute('data-async') || !target || method.toLowerCase() === 'dialog') {
      // A plain post that takes seconds (testing a connection) says so until its answer arrives.
      const waiting = submitter?.dataset.busyMessage || form.dataset.busyMessage
      if (waiting) toast(waiting, { sticky: true })
      holdPlain(form, submitter)
      return
    }
    event.preventDefault()
    submitAsync(form, submitter, target)
  })

  // The reset event fires before the controls change.
  on('reset', ({ target: form }) =>
    setTimeout(() => {
      syncForm(form)
      $$('[data-bind-source]', form).forEach((source) => syncBinding(source))
      syncSelection()
    })
  )

  on('formdata', sendUtc)
  // close, toggle and beforetoggle do not bubble: they are caught on the way down.
  on('close', ({ target }) => target.matches('dialog') && dialogClosed(target), true)
  on('toggle', (event) => event.target.matches('[popover]') && popoverToggled(event), true)
  if (!anchored) {
    // Measured in the frame the menu appears in, so it is never painted at the wrong place.
    const place = ({ target }) =>
      target.matches(MENUS) && requestAnimationFrame(() => placePopover(target))
    const replace = () => $$('[data-popover-positioned]:popover-open').forEach(placePopover)
    on('beforetoggle', place, true)
    document.addEventListener('scroll', replace, true)
    addEventListener('resize', replace)
  }

  addEventListener('hashchange', () => $$('[data-tabs="hash"]').forEach(restoreHashTab))
  // A page restored from the back/forward cache must not come back with a pending form.
  addEventListener('pageshow', (event) => {
    if (!event.persisted) return
    for (const form of $$('form')) {
      cancelPending(form)
      held.get(form)?.()
    }
  })
  setInterval(() => $$('time[data-format="relative"]').forEach(renderTime), 60000)

  if (document.readyState === 'loading')
    document.addEventListener('DOMContentLoaded', () => enhance())
  else enhance()
})()

/*
 * Greeting of the Home page. The server does not know the viewer's hour: it writes
 * "Welcome back", and the element whose text starts with those words names them in
 * data-greeting="Welcome back". They become "Good morning" (5 to 11), "Good afternoon"
 * (12 to 17) or "Good evening" on the viewer's clock. data-greeting then holds the new words, so
 * a second pass changes nothing.
 */
;(() => {
  function greet(heading) {
    const hour = new Date().getHours()
    const part = hour < 5 || hour >= 18 ? 'evening' : hour < 12 ? 'morning' : 'afternoon'
    const words = `Good ${part}`
    const text = heading.firstChild
    const shown = heading.dataset.greeting
    if (!shown || text?.nodeType !== Node.TEXT_NODE || !text.data.startsWith(shown)) return
    text.data = words + text.data.slice(shown.length)
    heading.dataset.greeting = words
  }

  // app:enhance follows each pass over the markup; the first pass may be over already.
  const greetAll = () => document.querySelectorAll('[data-greeting]').forEach(greet)
  document.addEventListener('app:enhance', greetAll)
  greetAll()
})()
