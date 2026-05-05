// Aura Browser JS bootstrap environment
// (Loaded at compile-time via include_bytes! in js.rs)

// -- Tracking state ----------------------------------------------------------
var __aura_style_log = [];
var __aura_inner_html_log = [];

function __aura_set_style(id, prop, value) {
    __aura_style_log.push(id + '||||' + prop + '||||' + value);
}

function __aura_url_parts(href) {
    var value = String(href || '');
    var parts = value.match(/^(([^:/?#]+):)?(\/\/([^/?#]*))?([^?#]*)(\?([^#]*))?(#(.*))?/);
    if (!parts) return null;
    var host = parts[4] || '';
    var hostParts = host.split(':');
    var protocol = (parts[2] || 'http') + ':';
    return {
        href: value,
        protocol: protocol,
        host: host,
        hostname: hostParts[0] || '',
        port: hostParts[1] || '',
        pathname: parts[5] || '/',
        search: parts[6] || '',
        hash: parts[8] || '',
        origin: protocol + '//' + host,
    };
}

function __aura_apply_location_href(href) {
    var parts = __aura_url_parts(href);
    if (!parts) return false;
    var loc = document.location;
    loc.href = parts.href;
    loc.protocol = parts.protocol;
    loc.host = parts.host;
    loc.hostname = parts.hostname;
    loc.port = parts.port;
    loc.pathname = parts.pathname;
    loc.search = parts.search;
    loc.hash = parts.hash;
    loc.origin = parts.origin;
    document.URL = parts.href;
    document.documentURI = parts.href;
    document.baseURI = parts.href;
    return true;
}

// -- Node Registry (Ensures stable objects for events) -----------------------
var __node_registry = new Map();

function __aura_node_kind_from_type(nodeType) {
    if (nodeType === 1) return 'element';
    if (nodeType === 3) return 'text';
    if (nodeType === 8) return 'comment';
    if (nodeType === 9) return 'document';
    if (nodeType === 10) return 'doctype';
    if (nodeType === 11) return 'fragment';
    return null;
}

function __aura_native_node_type(id) {
    if (!id || typeof __aura_get_node_type !== 'function') return 0;
    return __aura_get_node_type(id) || 0;
}

function __aura_get_node_descriptor(id, tag, string_id, kind) {
    let info = null;
    if (id && typeof __aura_get_node_info === 'function') {
        info = __aura_get_node_info(id);
    }

    let resolvedKind = kind || (info && info.kind) || __aura_node_kind_from_type(__aura_native_node_type(id)) || null;
    let resolvedTag = tag || (info && info.tag) || null;
    let resolvedId = string_id || (info && info.id) || null;

    if (!resolvedKind && resolvedTag) resolvedKind = 'element';

    return {
        tag: resolvedTag,
        id: resolvedId,
        kind: resolvedKind
    };
}

function __aura_read_character_data(id, kind) {
    if (!id) return '';
    if (typeof __aura_get_node_value === 'function') {
        let value = __aura_get_node_value(id);
        return value == null ? '' : String(value);
    }
    if (typeof __aura_get_character_data === 'function') {
        let value = __aura_get_character_data(id);
        return value == null ? '' : String(value);
    }
    if (kind === 'comment' && typeof __aura_get_comment_data === 'function') {
        let value = __aura_get_comment_data(id);
        return value == null ? '' : String(value);
    }
    return __aura_get_text_content(id);
}

function __aura_write_character_data(id, kind, value) {
    if (!id) return false;
    let text = String(value);
    if (typeof __aura_set_node_value === 'function') {
        __aura_set_node_value(id, text);
        return true;
    }
    if (typeof __aura_set_character_data === 'function') {
        __aura_set_character_data(id, text);
        return true;
    }
    if (kind === 'comment' && typeof __aura_set_comment_data === 'function') {
        __aura_set_comment_data(id, text);
        return true;
    }
    if (kind === 'text') {
        __aura_set_text_content(id, text);
        return true;
    }
    return false;
}

function __aura_read_document_type_info(id) {
    if (id && typeof __aura_get_document_type_info === 'function') {
        return __aura_get_document_type_info(id);
    }
    if (id && typeof __aura_get_doctype_info === 'function') {
        return __aura_get_doctype_info(id);
    }
    return null;
}

function __get_or_create_node(id, tag, string_id, kind) {
    if (!id) return null;
    if (__node_registry.has(id)) return __node_registry.get(id);
    let descriptor = __aura_get_node_descriptor(id, tag, string_id, kind);
    let node;
    if (descriptor.kind === 'text') {
        node = new TextNode(id);
    } else if (descriptor.kind === 'comment') {
        node = new Comment(id);
    } else if (descriptor.kind === 'fragment') {
        node = new DocumentFragment(id);
    } else if (descriptor.kind === 'doctype') {
        node = new DocumentType(id);
    } else if (descriptor.kind === 'document') {
        node = document;
    } else if (descriptor.tag) {
        node = new Element(id, descriptor.tag, descriptor.id);
    } else {
        node = new Node(id, descriptor.kind || 'element');
    }
    if (node !== document) {
        __node_registry.set(id, node);
    }
    return node;
}

// -- Event System ------------------------------------------------------------
class Event {
    constructor(type, options = {}) {
        this.type = type;
        this.bubbles = options.bubbles || false;
        this.cancelable = options.cancelable || false;
        this.composed = options.composed || false;
        this.target = null;
        this.currentTarget = null;
        this.eventPhase = 0;
        this.defaultPrevented = false;
        this._path = [];
        this._stopped = false;
        this._immediateStopped = false;
    }
    preventDefault() {
        if (this.cancelable) this.defaultPrevented = true;
    }
    stopPropagation() { this._stopped = true; }
    stopImmediatePropagation() { this._stopped = true; this._immediateStopped = true; }
    composedPath() { return this._path.slice(); }
}
Event.NONE = 0;
Event.CAPTURING_PHASE = 1;
Event.AT_TARGET = 2;
Event.BUBBLING_PHASE = 3;

class MouseEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.clientX = options.clientX || 0;
        this.clientY = options.clientY || 0;
    }
}

function __aura_normalize_listener_options(options) {
    if (options === true) return { capture: true, once: false };
    if (!options || options === false) return { capture: false, once: false };
    return {
        capture: !!options.capture,
        once: !!options.once
    };
}

function __aura_add_listener(target, type, callback, options) {
    if (typeof callback !== 'function') return;
    if (!target._listeners) target._listeners = new Map();
    if (!target._listeners.has(type)) target._listeners.set(type, []);
    let normalized = __aura_normalize_listener_options(options);
    target._listeners.get(type).push({
        callback: callback,
        capture: normalized.capture,
        once: normalized.once
    });
}

function __aura_remove_listener(target, type, callback, options) {
    if (!target._listeners || !target._listeners.has(type)) return;
    let normalized = __aura_normalize_listener_options(options);
    target._listeners.set(type, target._listeners.get(type).filter(function(listener) {
        return listener.callback !== callback || listener.capture !== normalized.capture;
    }));
}

function __aura_append_window_to_path(path) {
    if (path[path.length - 1] !== window) path.push(window);
}

function __aura_event_path(target) {
    let path = [target];
    if (target === window) return path;
    if (target === document) {
        __aura_append_window_to_path(path);
        return path;
    }
    let current = target;
    while (current && current.parentNode) {
        current = current.parentNode;
        path.push(current);
    }
    if (path[path.length - 1] !== document) path.push(document);
    __aura_append_window_to_path(path);
    return path;
}

function __aura_run_on_handler(target, event) {
    let onHandler = target && target['on' + event.type];
    if (typeof onHandler === 'function') {
        try { onHandler.call(target, event); } catch(e) { console.log("Event Error: " + e); }
    }
}

function __aura_invoke_listeners(target, event, capture) {
    let list = target && target._listeners ? target._listeners.get(event.type) : null;
    if (!list || list.length === 0) return;
    for (let listener of list.slice()) {
        if (event._immediateStopped) break;
        if (!!listener.capture !== !!capture) continue;
        try { listener.callback.call(target, event); } catch(e) { console.log("Event Error: " + e); }
        if (listener.once) {
            __aura_remove_listener(target, event.type, listener.callback, { capture: listener.capture });
        }
    }
}

function __aura_dispatch_event(target, event) {
    event.target = target;
    event.currentTarget = null;
    event.eventPhase = Event.NONE;
    event._stopped = false;
    event._immediateStopped = false;
    event._path = __aura_event_path(target);

    let path = event._path;
    for (let i = path.length - 1; i >= 1; i--) {
        if (event._stopped) break;
        let current = path[i];
        event.currentTarget = current;
        event.eventPhase = Event.CAPTURING_PHASE;
        __aura_invoke_listeners(current, event, true);
    }

    if (!event._stopped) {
        event.currentTarget = target;
        event.eventPhase = Event.AT_TARGET;
        __aura_invoke_listeners(target, event, true);
        if (!event._immediateStopped) {
            __aura_invoke_listeners(target, event, false);
            if (!event._immediateStopped) __aura_run_on_handler(target, event);
        }
    }

    if (event.bubbles && !event._stopped) {
        for (let i = 1; i < path.length; i++) {
            if (event._stopped) break;
            let current = path[i];
            event.currentTarget = current;
            event.eventPhase = Event.BUBBLING_PHASE;
            __aura_invoke_listeners(current, event, false);
            if (!event._immediateStopped) __aura_run_on_handler(current, event);
        }
    }

    event.currentTarget = null;
    event.eventPhase = Event.NONE;
    return !event.defaultPrevented;
}

class EventTarget {
    constructor() {
        this._listeners = new Map();
    }
    addEventListener(type, callback, options) {
        __aura_add_listener(this, type, callback, options);
    }
    removeEventListener(type, callback, options) {
        __aura_remove_listener(this, type, callback, options);
    }
    dispatchEvent(event) {
        return __aura_dispatch_event(this, event);
    }
}

// -- ClassList ----------------------------------------------------------------
class DOMTokenList {
    constructor(nid) {
        this._nid = nid;
    }
    _getClasses() {
        let cls = __aura_get_attribute(this._nid, 'class') || '';
        return cls.split(/\s+/).filter(c => c.length > 0);
    }
    _setClasses(arr) {
        __aura_set_attribute(this._nid, 'class', arr.join(' '));
    }
    add(...tokens) {
        let classes = this._getClasses();
        for (let t of tokens) {
            if (!classes.includes(t)) classes.push(t);
        }
        this._setClasses(classes);
    }
    remove(...tokens) {
        let classes = this._getClasses();
        for (let t of tokens) {
            classes = classes.filter(c => c !== t);
        }
        this._setClasses(classes);
    }
    toggle(token, force) {
        let classes = this._getClasses();
        let has = classes.includes(token);
        if (force === undefined) {
            if (has) {
                classes = classes.filter(c => c !== token);
            } else {
                classes.push(token);
            }
            this._setClasses(classes);
            return !has;
        } else {
            if (force) {
                if (!has) { classes.push(token); this._setClasses(classes); }
            } else {
                if (has) { classes = classes.filter(c => c !== token); this._setClasses(classes); }
            }
            return force;
        }
    }
    contains(token) {
        return this._getClasses().includes(token);
    }
    replace(oldToken, newToken) {
        let classes = this._getClasses();
        let idx = classes.indexOf(oldToken);
        if (idx !== -1) {
            classes[idx] = newToken;
            this._setClasses(classes);
            return true;
        }
        return false;
    }
    toString() {
        return this._getClasses().join(' ');
    }
    get length() { return this._getClasses().length; }
    item(index) { return this._getClasses()[index] || null; }
}

// -- NodeList / HTMLCollection ------------------------------------------------
class NodeList {
    constructor(nids, tag) {
        this._nids = nids.map(item => typeof item === 'object' ? item.nid : item);
        this._tag = tag;
        for (let i = 0; i < nids.length; i++) {
            let item = nids[i];
            if (typeof item === 'object') {
                this[i] = __get_or_create_node(item.nid, item.tag, item.id, item.kind);
            } else {
                let info = __aura_get_node_info(item);
                this[i] = info ? __get_or_create_node(item, info.tag, info.id, info.kind) : __get_or_create_node(item);
            }
        }
        this.length = nids.length;
    }
    item(i) { return this[i] || null; }
    forEach(fn) { this._nids.forEach((nid, i) => fn(this[i], i, this)); }
    [Symbol.iterator]() {
        let i = 0, self = this;
        return { next() { return i < self.length ? { value: self[i++], done: false } : { done: true }; } };
    }
}

function __aura_collection_node(item) {
    if (typeof item === 'object') {
        return __get_or_create_node(item.nid, item.tag, item.id, item.kind);
    }
    let info = __aura_get_node_info(item);
    return info ? __get_or_create_node(item, info.tag, info.id, info.kind) : __get_or_create_node(item);
}

class HTMLCollection {
    constructor(resolver) {
        this._resolver = resolver;
        return new Proxy(this, {
            get(target, prop, receiver) {
                if (prop === 'length') return target._items().length;
                if (prop === Symbol.iterator) return target[Symbol.iterator].bind(target);
                if (typeof prop === 'string') {
                    if (/^(0|[1-9]\d*)$/.test(prop)) return target.item(Number(prop));
                    if (!(prop in target)) {
                        let named = target.namedItem(prop);
                        if (named) return named;
                    }
                }
                let value = Reflect.get(target, prop, receiver);
                return typeof value === 'function' ? value.bind(target) : value;
            },
            has(target, prop) {
                if (typeof prop === 'string' && /^(0|[1-9]\d*)$/.test(prop)) {
                    return Number(prop) < target.length;
                }
                return prop in target;
            }
        });
    }
    _items() {
        return this._resolver();
    }
    item(index) {
        let item = this._items()[index];
        return item === undefined ? null : __aura_collection_node(item);
    }
    namedItem(name) {
        name = String(name);
        for (let item of this._items()) {
            let node = __aura_collection_node(item);
            if (!node || node.nodeType !== Node.ELEMENT_NODE) continue;
            if (node.id === name || node.getAttribute('name') === name) return node;
        }
        return null;
    }
    [Symbol.iterator]() {
        let i = 0;
        return {
            next: () => {
                let value = this.item(i++);
                return value ? { value, done: false } : { done: true };
            }
        };
    }
}

// -- Node & Element Classes ---------------------------------------------------

// Node type constants (static properties added after class definition)
class Node extends EventTarget {
    constructor(id, kind = 'element') {
        super();
        this._id = id;
        this._kind = kind;
    }
    get nodeType() {
        if (this._kind === 'element') return 1;
        if (this._kind === 'text') return 3;
        if (this._kind === 'comment') return 8;
        if (this._kind === 'document') return 9;
        if (this._kind === 'doctype') return 10;
        if (this._kind === 'fragment') return 11;
        return __aura_native_node_type(this._id) || 0;
    }
    get nodeName() {
        if (this._kind === 'text') return '#text';
        if (this._kind === 'comment') return '#comment';
        if (this._kind === 'document') return '#document';
        if (this._kind === 'fragment') return '#document-fragment';
        if (this._kind === 'doctype') return this.name || '';
        let info = this._id ? __aura_get_node_descriptor(this._id, null, null, this._kind) : null;
        if (info && info.tag) return info.tag.toUpperCase();
        return '';
    }
    get nodeValue() {
        return null;
    }
    get parentNode() {
        if (!this._id) return null;
        let pid = __aura_get_parent_id(this._id);
        if (!pid) return null;
        let info = __aura_get_node_info(pid);
        return info ? __get_or_create_node(pid, info.tag, info.id, info.kind) : __get_or_create_node(pid);
    }
    get parentElement() {
        let parent = this.parentNode;
        return parent && parent.nodeType === Node.ELEMENT_NODE ? parent : null;
    }
    get childNodes() {
        if (!this._id) return new NodeList([]);
        let arr = JSON.parse(__aura_get_children(this._id));
        return new NodeList(arr);
    }
    get firstChild() {
        if (!this._id) return null;
        let arr = JSON.parse(__aura_get_children(this._id));
        if (arr.length === 0) return null;
        let c = arr[0];
        return __get_or_create_node(c.nid, c.tag, c.id, c.kind);
    }
    get lastChild() {
        if (!this._id) return null;
        let arr = JSON.parse(__aura_get_children(this._id));
        if (arr.length === 0) return null;
        let c = arr[arr.length - 1];
        return __get_or_create_node(c.nid, c.tag, c.id, c.kind);
    }
    get textContent() {
        if (!this._id) return '';
        return __aura_get_text_content(this._id);
    }
    set textContent(val) {
        if (!this._id) return;
        __aura_set_text_content(this._id, String(val));
    }
    get nextSibling() {
        if (!this._id) return null;
        if (typeof __aura_get_next_sibling_id === 'function') {
            let nid = __aura_get_next_sibling_id(this._id);
            if (!nid) return null;
            let info = __aura_get_node_info(nid);
            return info ? __get_or_create_node(nid, info.tag, info.id, info.kind) : __get_or_create_node(nid);
        }
        let parent = this.parentNode;
        if (!parent) return null;
        let siblings = parent.childNodes;
        for (let i = 0; i < siblings.length - 1; i++) {
            if (siblings[i] && siblings[i]._id === this._id) return siblings[i + 1];
        }
        return null;
    }
    get previousSibling() {
        if (!this._id) return null;
        if (typeof __aura_get_previous_sibling_id === 'function') {
            let nid = __aura_get_previous_sibling_id(this._id);
            if (!nid) return null;
            let info = __aura_get_node_info(nid);
            return info ? __get_or_create_node(nid, info.tag, info.id, info.kind) : __get_or_create_node(nid);
        }
        let parent = this.parentNode;
        if (!parent) return null;
        let siblings = parent.childNodes;
        for (let i = 1; i < siblings.length; i++) {
            if (siblings[i] && siblings[i]._id === this._id) return siblings[i - 1];
        }
        return null;
    }
    get nextElementSibling() {
        if (!this._id) return null;
        if (typeof __aura_get_next_element_sibling_id === 'function') {
            let nid = __aura_get_next_element_sibling_id(this._id);
            if (!nid) return null;
            let info = __aura_get_node_info(nid);
            return info ? __get_or_create_node(nid, info.tag, info.id, info.kind) : __get_or_create_node(nid);
        }
        let node = this.nextSibling;
        while (node && node.nodeType !== Node.ELEMENT_NODE) node = node.nextSibling;
        return node;
    }
    get previousElementSibling() {
        if (!this._id) return null;
        if (typeof __aura_get_previous_element_sibling_id === 'function') {
            let nid = __aura_get_previous_element_sibling_id(this._id);
            if (!nid) return null;
            let info = __aura_get_node_info(nid);
            return info ? __get_or_create_node(nid, info.tag, info.id, info.kind) : __get_or_create_node(nid);
        }
        let node = this.previousSibling;
        while (node && node.nodeType !== Node.ELEMENT_NODE) node = node.previousSibling;
        return node;
    }
    appendChild(child) {
        if (child && child._id !== undefined && child._id !== null) {
            __aura_append_child(this._id, child._id);
        }
        return child;
    }
    removeChild(child) {
        if (child && child._id !== undefined && child._id !== null) {
            __aura_remove_child(this._id, child._id);
        }
        return child;
    }
    insertBefore(newChild, refChild) {
        if (newChild && newChild._id !== undefined && newChild._id !== null) {
            __aura_insert_before(this._id, newChild._id, refChild ? refChild._id : null);
        }
        return newChild;
    }
    replaceChild(newChild, oldChild) {
        if (newChild && newChild._id !== undefined && newChild._id !== null && oldChild && oldChild._id !== undefined && oldChild._id !== null) {
            __aura_insert_before(this._id, newChild._id, oldChild._id);
            __aura_remove_child(this._id, oldChild._id);
        }
        return oldChild;
    }
    cloneNode(deep) {
        if (this._kind === 'text') {
            return document.createTextNode(this.data);
        }
        if (this._kind === 'comment') {
            return document.createComment(this.data);
        }
        if (this._kind === 'doctype') {
            return new DocumentType(null, this.name, this.publicId, this.systemId);
        }
        if (this._kind === 'fragment') {
            let fragment = document.createDocumentFragment();
            if (deep) {
                let children = this.childNodes;
                for (let i = 0; i < children.length; i++) {
                    fragment.appendChild(children[i].cloneNode(true));
                }
            }
            return fragment;
        }
        let info = __aura_get_node_info(this._id);
        if (!info) return null;
        let clone = document.createElement(info.tag);
        let attrs = typeof __aura_get_attributes === 'function'
            ? JSON.parse(__aura_get_attributes(this._id))
            : [];
        for (let i = 0; i < attrs.length; i++) {
            clone.setAttribute(attrs[i].name, attrs[i].value);
        }
        if (deep) {
            let children = this.childNodes;
            for (let i = 0; i < children.length; i++) {
                clone.appendChild(children[i].cloneNode(true));
            }
        }
        return clone;
    }
    contains(other) {
        if (!other || !other._id) return false;
        if (other._id === this._id) return true;
        let children = JSON.parse(__aura_get_children(this._id));
        for (let c of children) {
            let child_node = __get_or_create_node(c.nid, c.tag, c.id, c.kind);
            if (child_node.contains(other)) return true;
        }
        return false;
    }
}

class Element extends Node {
    constructor(id, tag, string_id) {
        super(id, 'element');
        this.tagName = (tag || '').toUpperCase();
        this.id = string_id || '';
        this._classList = null;
        this.style = new Proxy({ _id: id }, {
            set: (target, prop, value) => {
                let kebab = prop.replace(/([A-Z])/g, "-$1").toLowerCase();
                __aura_set_style(target._id, kebab, value);
                target[prop] = value;
                return true;
            },
            get: (target, prop) => {
                return target[prop];
            }
        });
    }
    get classList() {
        if (!this._classList) this._classList = new DOMTokenList(this._id);
        return this._classList;
    }
    get className() {
        return __aura_get_attribute(this._id, 'class') || '';
    }
    set className(val) {
        __aura_set_attribute(this._id, 'class', String(val));
    }
    get innerHTML() {
        return __aura_get_inner_html(this._id);
    }
    set innerHTML(val) {
        __aura_set_inner_html(this._id, String(val));
    }
    get outerHTML() {
        if (typeof __aura_get_outer_html === 'function') {
            return __aura_get_outer_html(this._id);
        }
        let info = __aura_get_node_info(this._id);
        if (!info) return '';
        let inner = __aura_get_inner_html(this._id);
        let tag = info.tag.toLowerCase();
        let cls = info.class ? ` class="${info.class}"` : '';
        let id_attr = info.id ? ` id="${info.id}"` : '';
        return `<${tag}${id_attr}${cls}>${inner}</${tag}>`;
    }
    get textContent() {
        return __aura_get_text_content(this._id);
    }
    set textContent(val) {
        __aura_set_text_content(this._id, String(val));
    }
    setAttribute(name, value) {
        __aura_set_attribute(this._id, name, String(value));
    }
    getAttribute(name) {
        return __aura_get_attribute(this._id, name);
    }
    removeAttribute(name) {
        __aura_remove_attribute(this._id, name);
    }
    hasAttribute(name) {
        return __aura_has_attribute(this._id, name);
    }
    remove() {
        __aura_remove_self(this._id);
    }
    focus() {
        __aura_set_focus(this.id);
    }
    blur() {}
    matches(selector) {
        // Use querySelector from root to check if this element matches
        // Simplified: just check tag/class/id
        let nids_json = __aura_query_selector_all(0, selector);
        let nids = JSON.parse(nids_json);
        return nids.includes(this._id);
    }
    closest(selector) {
        let node = this;
        while (node) {
            if (node.matches && node.matches(selector)) return node;
            node = node.parentNode;
        }
        return null;
    }
    querySelector(selector) {
        let nid = __aura_query_selector(this._id, selector);
        if (!nid) return null;
        let info = __aura_get_node_info(nid);
        return info ? __get_or_create_node(nid, info.tag, info.id, info.kind) : __get_or_create_node(nid);
    }
    querySelectorAll(selector) {
        let nids_json = __aura_query_selector_all(this._id, selector);
        let nids = JSON.parse(nids_json);
        return new NodeList(nids.map(nid => {
            let info = __aura_get_node_info(nid);
            return __get_or_create_node(nid, info ? info.tag : null, info ? info.id : null, info ? info.kind : null);
        }).map(el => el._id));
    }
    getElementsByClassName(cls) {
        let rootId = this._id;
        let className = String(cls);
        return new HTMLCollection(function() {
            return JSON.parse(__aura_get_elements_by_class(rootId, className));
        });
    }
    getElementsByTagName(tag) {
        let rootId = this._id;
        let tagName = String(tag).toLowerCase();
        return new HTMLCollection(function() {
            return JSON.parse(__aura_get_elements_by_tag(rootId, tagName));
        });
    }
    get children() {
        let rootId = this._id;
        return new HTMLCollection(function() {
            return JSON.parse(__aura_get_children(rootId)).filter(c => c.kind === 'element');
        });
    }
    get childElementCount() {
        let arr = JSON.parse(__aura_get_children(this._id));
        return arr.filter(c => c.kind === 'element').length;
    }
    get firstElementChild() {
        let arr = JSON.parse(__aura_get_children(this._id));
        let c = arr.find(c => c.kind === 'element');
        return c ? __get_or_create_node(c.nid, c.tag, c.id, c.kind) : null;
    }
    get lastElementChild() {
        let arr = JSON.parse(__aura_get_children(this._id));
        var c = null;
        for (let i = arr.length - 1; i >= 0; i--) {
            if (arr[i].kind === 'element') { c = arr[i]; break; }
        }
        return c ? __get_or_create_node(c.nid, c.tag, c.id, c.kind) : null;
    }
    _layoutMetrics() {
        if (typeof __aura_get_layout_metrics === 'function') {
            let metrics = __aura_get_layout_metrics(this._id);
            if (metrics) return metrics;
        }
        return { x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0 };
    }
    getBoundingClientRect() {
        return this._layoutMetrics();
    }
    getClientRects() {
        let rect = this._layoutMetrics();
        return rect.width > 0 || rect.height > 0 ? [rect] : [];
    }
    // scrollIntoView stub
    scrollIntoView() {}
    scroll() {}
    scrollTo() {}
    scrollBy() {}
    // nodeType for Element is always ELEMENT_NODE (1)
    get nodeType() { return 1; }
    // insertAdjacentHTML: inject HTML relative to this element
    insertAdjacentHTML(position, html) {
        position = position.toLowerCase();
        if (position === 'beforeend' || position === 'afterbegin') {
            // Append/prepend to children
            var frag = document.createElement('div');
            frag.innerHTML = html;
            var children = JSON.parse(__aura_get_children(frag._id));
            for (var i = 0; i < children.length; i++) {
                var child = __get_or_create_node(children[i].nid, children[i].tag, children[i].id, children[i].kind);
                if (position === 'beforeend') {
                    this.appendChild(child);
                } else {
                    this.insertBefore(child, this.firstChild);
                }
            }
        }
        // 'beforebegin' and 'afterend' require parent access — stub as no-op
    }
    insertAdjacentText(position, text) {
        // Simplified: treat as insertAdjacentHTML with escaped text
    }
    insertAdjacentElement(position, element) {
        position = position.toLowerCase();
        if (position === 'beforeend') this.appendChild(element);
        return element;
    }
    // dataset stub
    get dataset() {
        var self = this;
        return new Proxy({}, {
            get: function(target, key) {
                var attrName = 'data-' + key.replace(/([A-Z])/g, '-$1').toLowerCase();
                return __aura_get_attribute(self._id, attrName);
            },
            set: function(target, key, value) {
                var attrName = 'data-' + key.replace(/([A-Z])/g, '-$1').toLowerCase();
                __aura_set_attribute(self._id, attrName, String(value));
                return true;
            }
        });
    }
    // checked / value properties (for input elements)
    get value() { return __aura_get_attribute(this._id, 'value') || ''; }
    set value(v) { __aura_set_attribute(this._id, 'value', String(v)); }
    get checked() { return __aura_has_attribute(this._id, 'checked'); }
    set checked(v) {
        if (v) __aura_set_attribute(this._id, 'checked', 'checked');
        else __aura_remove_attribute(this._id, 'checked');
    }
    get disabled() { return __aura_has_attribute(this._id, 'disabled'); }
    set disabled(v) {
        if (v) __aura_set_attribute(this._id, 'disabled', 'disabled');
        else __aura_remove_attribute(this._id, 'disabled');
    }
    get href() { return __aura_get_attribute(this._id, 'href') || ''; }
    set href(v) { __aura_set_attribute(this._id, 'href', String(v)); }
    get src() { return __aura_get_attribute(this._id, 'src') || ''; }
    set src(v) { __aura_set_attribute(this._id, 'src', String(v)); }
    get type() { return __aura_get_attribute(this._id, 'type') || ''; }
    set type(v) { __aura_set_attribute(this._id, 'type', String(v)); }
    get name() { return __aura_get_attribute(this._id, 'name') || ''; }
    set name(v) { __aura_set_attribute(this._id, 'name', String(v)); }
    // Form select stubs
    get selectedIndex() { return -1; }
    set selectedIndex(v) {}
    get options() { return new NodeList([]); }
    get offsetWidth() { return this._layoutMetrics().width; }
    get offsetHeight() { return this._layoutMetrics().height; }
    get offsetTop() { return this._layoutMetrics().top; }
    get offsetLeft() { return this._layoutMetrics().left; }
    get scrollWidth() { return this._layoutMetrics().width; }
    get scrollHeight() { return this._layoutMetrics().height; }
    get scrollTop() { return 0; }
    set scrollTop(v) {}
    get scrollLeft() { return 0; }
    set scrollLeft(v) {}
    get clientWidth() { return this._layoutMetrics().width; }
    get clientHeight() { return this._layoutMetrics().height; }
    // namespaceURI
    get namespaceURI() { return 'http://www.w3.org/1999/xhtml'; }
    // setAttributeNS / getAttributeNS / removeAttributeNS
    setAttributeNS(ns, name, value) { this.setAttribute(name, value); }
    getAttributeNS(ns, name) { return this.getAttribute(name); }
    removeAttributeNS(ns, name) { this.removeAttribute(name); }
    hasAttributeNS(ns, name) { return this.hasAttribute(name); }
    // attachShadow stub
    attachShadow(options) {
        var host = this;
        var shadow = document.createElement('div');
        shadow._isShadowRoot = true;
        shadow.host = host;
        host._shadowRoot = shadow;
        return shadow;
    }
    get shadowRoot() { return this._shadowRoot || null; }
    // dispatchEvent is inherited from EventTarget; no override needed
    // animate stub
    animate(keyframes, options) {
        return {
            play: function() {},
            pause: function() {},
            cancel: function() {},
            finish: function() {},
            addEventListener: function() {},
            removeEventListener: function() {},
            finished: Promise.resolve(),
            ready: Promise.resolve(),
            playState: 'finished',
        };
    }
}

class TextNode extends Node {
    constructor(id) {
        super(id, 'text');
        this._data = '';
    }
    get data() {
        return this._id ? __aura_read_character_data(this._id, 'text') : this._data;
    }
    set data(val) {
        let text = String(val);
        if (!__aura_write_character_data(this._id, 'text', text)) {
            this._data = text;
        }
    }
    get nodeValue() {
        return this.data;
    }
    set nodeValue(val) {
        this.data = val;
    }
    get textContent() {
        return this.data;
    }
    set textContent(val) {
        this.data = val;
    }
}

class Comment extends Node {
    constructor(id, data) {
        super(id, 'comment');
        this._data = data === undefined ? '' : String(data);
    }
    get data() {
        return this._id ? __aura_read_character_data(this._id, 'comment') : this._data;
    }
    set data(val) {
        let text = String(val);
        if (!__aura_write_character_data(this._id, 'comment', text)) {
            this._data = text;
        }
    }
    get nodeValue() {
        return this.data;
    }
    set nodeValue(val) {
        this.data = val;
    }
    get textContent() {
        return this.data;
    }
    set textContent(val) {
        this.data = val;
    }
}

class DocumentType extends Node {
    constructor(id, name, publicId, systemId) {
        super(id, 'doctype');
        this._name = name || '';
        this._publicId = publicId || '';
        this._systemId = systemId || '';
    }
    _info() {
        return this._id ? __aura_read_document_type_info(this._id) : null;
    }
    get name() {
        let info = this._info();
        return info && info.name !== undefined ? String(info.name) : this._name;
    }
    get publicId() {
        let info = this._info();
        return info && info.publicId !== undefined ? String(info.publicId) : this._publicId;
    }
    get systemId() {
        let info = this._info();
        return info && info.systemId !== undefined ? String(info.systemId) : this._systemId;
    }
    get nodeValue() {
        return null;
    }
    get textContent() {
        return null;
    }
    set textContent(val) {}
    get nextSibling() {
        return super.nextSibling;
    }
}

class DocumentFragment extends Node {
    constructor(id) {
        super(id, 'fragment');
    }
    get nodeName() {
        return '#document-fragment';
    }
}

// -- Node static constants ---------------------------------------------------
Node.ELEMENT_NODE = 1;
Node.ATTRIBUTE_NODE = 2;
Node.TEXT_NODE = 3;
Node.CDATA_SECTION_NODE = 4;
Node.COMMENT_NODE = 8;
Node.DOCUMENT_NODE = 9;
Node.DOCUMENT_TYPE_NODE = 10;
Node.DOCUMENT_FRAGMENT_NODE = 11;

// -- Storage -----------------------------------------------------------------
var localStorage = {
    getItem: function(key) {
        return __aura_storage_get(String(key));
    },
    setItem: function(key, value) {
        __aura_storage_set(String(key), String(value));
    },
    removeItem: function(key) {
        __aura_storage_remove(String(key));
    },
    clear: function() {
        __aura_storage_clear();
    }
};

// -- document -----------------------------------------------------------------
var _document_listeners = new Map();

var document = {
    // Event listener support on document itself
    _listeners: new Map(),
    addEventListener: function(type, callback, options) {
        __aura_add_listener(this, type, callback, options);
        // DOMContentLoaded fires immediately (page already loaded)
        if (type === 'DOMContentLoaded') {
            try { callback(new Event('DOMContentLoaded')); } catch(e) {}
        }
    },
    removeEventListener: function(type, callback, options) {
        __aura_remove_listener(this, type, callback, options);
    },
    dispatchEvent: function(event) {
        return __aura_dispatch_event(this, event);
    },

    getElementById: function(id) {
        let res = __aura_get_element_by_id(id);
        return res ? __get_or_create_node(res.nid, res.tag, id, res.kind) : null;
    },
    createElement: function(tag) {
        let nativeId = __aura_create_element(tag);
        return __get_or_create_node(nativeId, tag, null, 'element');
    },
    createTextNode: function(text) {
        let nativeId = __aura_create_text_node(String(text));
        return __get_or_create_node(nativeId, null, null, 'text');
    },
    createComment: function(data) {
        let text = String(data);
        if (typeof __aura_create_comment_node === 'function') {
            let nativeId = __aura_create_comment_node(text);
            let comment = __get_or_create_node(nativeId, null, null, 'comment');
            comment._data = text;
            return comment;
        }
        if (typeof __aura_create_comment === 'function') {
            let nativeId = __aura_create_comment(text);
            let comment = __get_or_create_node(nativeId, null, null, 'comment');
            comment._data = text;
            return comment;
        }
        return new Comment(null, text);
    },
    createDocumentFragment: function() {
        let nativeId = __aura_create_document_fragment();
        return __get_or_create_node(nativeId, null, null, 'fragment');
    },

    querySelector: function(selector) {
        let nid = __aura_query_selector(0, selector);
        if (!nid) return null;
        let info = __aura_get_node_info(nid);
        return info ? __get_or_create_node(nid, info.tag, info.id, info.kind) : __get_or_create_node(nid);
    },
    querySelectorAll: function(selector) {
        let nids_json = __aura_query_selector_all(0, selector);
        let nids = JSON.parse(nids_json);
        let nodes = nids.map(nid => {
            let info = __aura_get_node_info(nid);
            return __get_or_create_node(nid, info ? info.tag : null, info ? info.id : null, info ? info.kind : null);
        });
        return new NodeList(nodes.map(n => n._id));
    },
    getElementsByClassName: function(cls) {
        let className = String(cls);
        return new HTMLCollection(function() {
            return JSON.parse(__aura_get_elements_by_class(0, className));
        });
    },
    getElementsByTagName: function(tag) {
        let tagName = String(tag).toLowerCase();
        return new HTMLCollection(function() {
            return JSON.parse(__aura_get_elements_by_tag(0, tagName));
        });
    },
    // Document surface getters follow the parsed tree shape, not arbitrary descendants.
    get body() {
        let nativeId = __aura_get_body();
        return nativeId ? __get_or_create_node(nativeId, 'body', null, 'element') : null;
    },
    get head() {
        let nativeId = __aura_get_head();
        return nativeId ? __get_or_create_node(nativeId, 'head', null, 'element') : null;
    },
    get documentElement() {
        let nativeId = __aura_get_document_element();
        return nativeId ? __get_or_create_node(nativeId, 'html', null, 'element') : null;
    },
    get doctype() {
        let nativeId = 0;
        if (typeof __aura_get_document_type === 'function') {
            nativeId = __aura_get_document_type() || 0;
        } else if (typeof __aura_get_doctype === 'function') {
            nativeId = __aura_get_doctype() || 0;
        }
        return nativeId ? __get_or_create_node(nativeId, null, null, 'doctype') : null;
    },
    activeElement: null,
    location: { href: '', hostname: '', pathname: '/', search: '', hash: '', protocol: 'https:', host: '', port: '', origin: '' },
    title: '',
    readyState: 'complete',
    referrer: '',
    characterSet: 'UTF-8',
    charset: 'UTF-8',
    inputEncoding: 'UTF-8',
    contentType: 'text/html',
    URL: '',
    documentURI: '',
    baseURI: '',
    nodeType: 9,
    nodeName: '#document',
    nodeValue: null,

    // createTreeWalker stub
    createTreeWalker: function(root, whatToShow, filter, expandEntityReferences) {
        var nodes = [];
        function collect(node) {
            nodes.push(node);
            var children = node && node.childNodes ? node.childNodes : [];
            for (var i = 0; i < children.length; i++) {
                collect(children[i]);
            }
        }
        if (root && root._id) collect(root);
        var i = 0;
        return {
            currentNode: root,
            nextNode: function() {
                if (i < nodes.length) { this.currentNode = nodes[i++]; return this.currentNode; }
                return null;
            },
            previousNode: function() {
                if (i > 0) { this.currentNode = nodes[--i]; return this.currentNode; }
                return null;
            },
            firstChild: function() { return null; },
            lastChild: function() { return null; },
            nextSibling: function() { return null; },
            previousSibling: function() { return null; },
            parentNode: function() { return null; },
        };
    },

    // createNodeIterator stub
    createNodeIterator: function(root, whatToShow, filter) {
        var nodes = [];
        function collect(node) {
            nodes.push(node);
            var children = node && node.childNodes ? node.childNodes : [];
            for (var i = 0; i < children.length; i++) {
                collect(children[i]);
            }
        }
        if (root && root._id) collect(root);
        var i = 0;
        return {
            nextNode: function() { return i < nodes.length ? nodes[i++] : null; },
            previousNode: function() { return i > 0 ? nodes[--i] : null; },
            detach: function() {},
        };
    },

    // createRange stub
    createRange: function() {
        return {
            setStart: function() {},
            setEnd: function() {},
            selectNode: function() {},
            selectNodeContents: function() {},
            collapse: function() {},
            cloneRange: function() { return document.createRange(); },
            deleteContents: function() {},
            extractContents: function() { return document.createDocumentFragment(); },
            insertNode: function() {},
            surroundContents: function() {},
            getBoundingClientRect: function() { return { x:0, y:0, width:0, height:0, top:0, left:0, right:0, bottom:0 }; },
            getClientRects: function() { return []; },
            toString: function() { return ''; },
            commonAncestorContainer: null,
        };
    },

    // execCommand stub
    execCommand: function(cmd, showUI, value) { return false; },
    queryCommandEnabled: function(cmd) { return false; },
    queryCommandSupported: function(cmd) { return false; },

    // importNode stub
    importNode: function(node, deep) { return node; },
    adoptNode: function(node) { return node; },

    // hasFocus stub
    hasFocus: function() { return false; },

    // visibilityState
    visibilityState: 'visible',
    hidden: false,

    // fullscreenElement
    fullscreenElement: null,
    fullscreenEnabled: false,

    // pointerLockElement
    pointerLockElement: null,

    // forms / images / scripts / links collections
    get forms() { return new NodeList([]); },
    get images() { return new NodeList([]); },
    get scripts() {
        let nids_json = __aura_get_elements_by_tag(0, 'script');
        let nids = JSON.parse(nids_json);
        return new NodeList(nids);
    },
    get links() {
        let nids_json = __aura_get_elements_by_tag(0, 'a');
        let nids = JSON.parse(nids_json);
        return new NodeList(nids);
    },

    // Internal bridge for Rust to trigger events
    __trigger_event: function(id, type, data) {
        let target = __get_or_create_node(id);
        if (target) {
            let ev = type.startsWith('mouse') ? new MouseEvent(type, data) : new Event(type, data);
            target.dispatchEvent(ev);
        }
    }
};

var window = globalThis;
window.document = document;
window.localStorage = localStorage;
var console = { log: log, warn: warn, error: error, info: info, debug: debug };
var navigator = {
    userAgent: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
    language: 'en-US',
    languages: ['en-US', 'en'],
    platform: 'Linux x86_64',
    cookieEnabled: false,
    onLine: true,
    hardwareConcurrency: 4,
    maxTouchPoints: 0,
    vendor: 'Google Inc.',
    vendorSub: '',
    productSub: '20030107',
};
var location = document.location;

// -- window dimensions -------------------------------------------------------
window.innerWidth = 800;
window.innerHeight = 600;
window.outerWidth = 800;
window.outerHeight = 600;
window.screen = {
    width: 800,
    height: 600,
    availWidth: 800,
    availHeight: 600,
    colorDepth: 24,
    pixelDepth: 24,
    orientation: { type: 'landscape-primary', angle: 0 },
};
window.devicePixelRatio = 1;
window.scrollX = 0;
window.scrollY = 0;
window.pageXOffset = 0;
window.pageYOffset = 0;
window.visualViewport = {
    width: 800,
    height: 600,
    scale: 1,
    offsetLeft: 0,
    offsetTop: 0,
    pageLeft: 0,
    pageTop: 0,
    addEventListener: function() {},
    removeEventListener: function() {},
};

// -- history -----------------------------------------------------------------
window.history = {
    length: 1,
    state: null,
    _entries: [location.href],
    _index: 0,
    pushState: function(state, title, url) {
        this.state = state;
        var nextHref = location.href;
        if (url !== undefined && url !== null) {
            try {
                nextHref = new URL(String(url), location.href).href;
            } catch (e) {
                nextHref = location.href;
            }
            __aura_apply_location_href(nextHref);
        }
        this._entries = this._entries.slice(0, this._index + 1);
        this._entries.push(nextHref);
        this._index = this._entries.length - 1;
        this.length = this._entries.length;
    },
    replaceState: function(state, title, url) {
        this.state = state;
        if (url !== undefined && url !== null) {
            var resolved;
            try {
                resolved = new URL(String(url), location.href).href;
            } catch (e) {
                resolved = location.href;
            }
            if (__aura_apply_location_href(resolved)) {
                this._entries[this._index] = location.href;
            }
        }
    },
    back: function() {},
    forward: function() {},
    go: function() {},
};

// -- performance -------------------------------------------------------------
window.performance = {
    _start: Date.now(),
    now: function() { return Date.now() - this._start; },
    mark: function() {},
    measure: function() {},
    getEntries: function() { return []; },
    getEntriesByName: function() { return []; },
    getEntriesByType: function() { return []; },
    clearMarks: function() {},
    clearMeasures: function() {},
    timing: {
        navigationStart: Date.now(),
        loadEventEnd: Date.now(),
        domContentLoadedEventEnd: Date.now(),
    },
    navigation: { type: 0, redirectCount: 0 },
    timeOrigin: Date.now(),
};

// -- document.cookie (stub, read-only empty string) --------------------------
Object.defineProperty(document, 'cookie', {
    get: function() { return ''; },
    set: function(val) { /* stub: ignore cookie writes */ },
    configurable: true,
});

// -- sessionStorage ----------------------------------------------------------
var sessionStorage = (function() {
    var _store = {};
    return {
        getItem: function(key) { return key in _store ? _store[key] : null; },
        setItem: function(key, value) { _store[String(key)] = String(value); },
        removeItem: function(key) { delete _store[key]; },
        clear: function() { _store = {}; },
        get length() { return Object.keys(_store).length; },
        key: function(i) { return Object.keys(_store)[i] || null; },
    };
})();
window.sessionStorage = sessionStorage;

// -- getComputedStyle --------------------------------------------------------
window.getComputedStyle = function(el, pseudoElt) {
    // Stub: returns an object with all properties returning empty string
    return new Proxy({}, {
        get: function(target, prop) {
            if (prop === 'getPropertyValue') {
                return function(name) { return ''; };
            }
            if (prop === 'length') return 0;
            if (prop === 'item') return function() { return ''; };
            return '';
        }
    });
};

// -- XMLHttpRequest stub -----------------------------------------------------
class XMLHttpRequest extends EventTarget {
    constructor() {
        super();
        this.readyState = 0;
        this.status = 0;
        this.statusText = '';
        this.responseText = '';
        this.response = null;
        this.responseType = '';
        this.timeout = 0;
        this.withCredentials = false;
        this.onreadystatechange = null;
        this.onload = null;
        this.onerror = null;
        this.ontimeout = null;
        this.onprogress = null;
        this.onabort = null;
        this._method = '';
        this._url = '';
        this._headers = {};
    }
    open(method, url, async) {
        this._method = method;
        this._url = url;
        this.readyState = 1;
    }
    send(body) {
        // Stub: fire error asynchronously
        var self = this;
        setTimeout(function() {
            self.readyState = 4;
            self.status = 0;
            if (typeof self.onerror === 'function') self.onerror(new Event('error'));
            if (typeof self.onreadystatechange === 'function') self.onreadystatechange();
        }, 0);
    }
    setRequestHeader(name, value) {
        this._headers[name] = value;
    }
    getResponseHeader(name) { return null; }
    getAllResponseHeaders() { return ''; }
    abort() {
        this.readyState = 0;
        if (typeof this.onabort === 'function') this.onabort(new Event('abort'));
    }
    overrideMimeType() {}
}
window.XMLHttpRequest = XMLHttpRequest;

// -- window.matchMedia -------------------------------------------------------
window.matchMedia = function(query) {
    return {
        matches: false,
        media: query,
        onchange: null,
        addListener: function() {},
        removeListener: function() {},
        addEventListener: function() {},
        removeEventListener: function() {},
        dispatchEvent: function() { return false; },
    };
};

// -- window.getSelection -----------------------------------------------------
window.getSelection = function() {
    return {
        rangeCount: 0,
        isCollapsed: true,
        toString: function() { return ''; },
        getRangeAt: function() { return null; },
        addRange: function() {},
        removeAllRanges: function() {},
        collapse: function() {},
    };
};

// -- window.crypto (basic stub) ----------------------------------------------
window.crypto = {
    getRandomValues: function(arr) {
        for (var i = 0; i < arr.length; i++) {
            arr[i] = Math.floor(Math.random() * 256);
        }
        return arr;
    },
    subtle: null,
};

// -- window.open, window.close -----------------------------------------------
window.open = function() { return null; };
window.close = function() {};
window.focus = function() {};
window.blur = function() {};

// -- Timers ------------------------------------------------------------------
window.setTimeout = function(fn, delay) {
    if (typeof fn === 'function') {
        __aura_queue_task(fn);
    } else if (typeof fn === 'string') {
        __aura_queue_task(() => eval(fn));
    }
    return 1;
};

window.clearTimeout = function() {};
window.setInterval = function(fn, delay) {
    // Simplified: run once as macro task
    if (typeof fn === 'function') {
        __aura_queue_task(fn);
    }
    return 1;
};
window.clearInterval = function() {};

// -- fetch() -----------------------------------------------------------------
window.fetch = function(url) {
    return new Promise((resolve, reject) => {
        __aura_fetch(url, resolve, reject);
    });
};

// -- MutationObserver stub ---------------------------------------------------
class MutationObserver {
    constructor(callback) { this._callback = callback; }
    observe(target, options) {}
    disconnect() {}
    takeRecords() { return []; }
}

// -- IntersectionObserver stub -----------------------------------------------
class IntersectionObserver {
    constructor(callback, options) { this._callback = callback; }
    observe(target) {}
    unobserve(target) {}
    disconnect() {}
}

// -- ResizeObserver stub -----------------------------------------------------
class ResizeObserver {
    constructor(callback) { this._callback = callback; }
    observe(target) {}
    unobserve(target) {}
    disconnect() {}
}

// -- CustomEvent -------------------------------------------------------------
class CustomEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.detail = options.detail !== undefined ? options.detail : null;
    }
}

// -- FocusEvent --------------------------------------------------------------
class FocusEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.relatedTarget = options.relatedTarget || null;
    }
}

// -- KeyboardEvent -----------------------------------------------------------
class KeyboardEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.key = options.key || '';
        this.code = options.code || '';
        this.keyCode = options.keyCode || 0;
        this.charCode = options.charCode || 0;
        this.which = options.which || this.keyCode || this.charCode || 0;
        this.altKey = options.altKey || false;
        this.ctrlKey = options.ctrlKey || false;
        this.shiftKey = options.shiftKey || false;
        this.metaKey = options.metaKey || false;
        this.repeat = options.repeat || false;
        this.location = options.location || 0;
    }
    getModifierState(key) { return false; }
}

// -- InputEvent --------------------------------------------------------------
class InputEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.data = options.data !== undefined ? options.data : null;
        this.inputType = options.inputType || '';
    }
}

// -- TouchEvent / PointerEvent stubs -----------------------------------------
class TouchEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.touches = options.touches || [];
        this.targetTouches = options.targetTouches || [];
        this.changedTouches = options.changedTouches || [];
    }
}

class PointerEvent extends MouseEvent {
    constructor(type, options = {}) {
        super(type, options);
        this.pointerId = options.pointerId || 1;
        this.pointerType = options.pointerType || 'mouse';
        this.pressure = options.pressure || 0;
        this.isPrimary = options.isPrimary !== undefined ? options.isPrimary : true;
    }
}

class WheelEvent extends MouseEvent {
    constructor(type, options = {}) {
        super(type, options);
        this.deltaX = options.deltaX || 0;
        this.deltaY = options.deltaY || 0;
        this.deltaZ = options.deltaZ || 0;
        this.deltaMode = options.deltaMode || 0;
    }
}

// -- UIEvent ----------------------------------------------------------------
class UIEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.detail = options.detail || 0;
    }
}

// -- ErrorEvent -------------------------------------------------------------
class ErrorEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.message = options.message || '';
        this.filename = options.filename || '';
        this.lineno = options.lineno || 0;
        this.colno = options.colno || 0;
        this.error = options.error || null;
    }
}

// -- MessageEvent -----------------------------------------------------------
class MessageEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.data = options.data !== undefined ? options.data : null;
        this.origin = options.origin || '';
        this.source = options.source || null;
    }
}

// -- ProgressEvent ----------------------------------------------------------
class ProgressEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.lengthComputable = options.lengthComputable || false;
        this.loaded = options.loaded || 0;
        this.total = options.total || 0;
    }
}

// -- StorageEvent -----------------------------------------------------------
class StorageEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.key = options.key || null;
        this.oldValue = options.oldValue || null;
        this.newValue = options.newValue || null;
        this.url = options.url || '';
        this.storageArea = options.storageArea || null;
    }
}

// -- HashChangeEvent --------------------------------------------------------
class HashChangeEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.oldURL = options.oldURL || '';
        this.newURL = options.newURL || '';
    }
}

// -- PopStateEvent ----------------------------------------------------------
class PopStateEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.state = options.state !== undefined ? options.state : null;
    }
}

// -- BeforeUnloadEvent ------------------------------------------------------
class BeforeUnloadEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.returnValue = '';
    }
}

// -- window global event handlers -------------------------------------------
window.onload = null;
window.onerror = null;
window.onunload = null;
window.onbeforeunload = null;
window.onhashchange = null;
window.onpopstate = null;
window.onmessage = null;
window.onresize = null;
window.onscroll = null;

// window.addEventListener wrapping around document events for common cases
var _window_listeners = new Map();
window.addEventListener = function(type, callback, options) {
    __aura_add_listener(window, type, callback, options);
    // Load event fires immediately (page already done)
    if (type === 'load' || type === 'DOMContentLoaded') {
        try { callback(new Event(type)); } catch(e) {}
    }
};
window.removeEventListener = function(type, callback, options) {
    __aura_remove_listener(window, type, callback, options);
};
window.dispatchEvent = function(event) {
    return __aura_dispatch_event(window, event);
};

// -- URL class stub ----------------------------------------------------------
class URL {
    constructor(url, base) {
        var parsed = __aura_parse_url(
            String(url),
            base === undefined || base === null || base === '' ? null : String(base)
        );
        if (!parsed) throw new TypeError('Invalid URL');
        this.href = parsed.href || '';
        this.protocol = parsed.protocol || '';
        this.host = parsed.host || '';
        this.hostname = parsed.hostname || '';
        this.port = parsed.port || '';
        this.pathname = parsed.pathname || '/';
        this.search = parsed.search || '';
        this.hash = parsed.hash || '';
        this.origin = parsed.origin || '';
        this.username = '';
        this.password = '';
    }
    toString() { return this.href; }
    static createObjectURL(blob) { return 'blob:'; }
    static revokeObjectURL(url) {}
}
window.URL = URL;

// -- URLSearchParams stub ----------------------------------------------------
class URLSearchParams {
    constructor(init) {
        this._params = {};
        if (typeof init === 'string') {
            init.replace(/^\?/, '').split('&').forEach(function(pair) {
                var parts = pair.split('=');
                if (parts[0]) {
                    this._params[decodeURIComponent(parts[0])] = decodeURIComponent(parts[1] || '');
                }
            }.bind(this));
        }
    }
    get(key) { return key in this._params ? this._params[key] : null; }
    set(key, value) { this._params[key] = String(value); }
    has(key) { return key in this._params; }
    delete(key) { delete this._params[key]; }
    toString() {
        return Object.keys(this._params).map(k => encodeURIComponent(k) + '=' + encodeURIComponent(this._params[k])).join('&');
    }
    forEach(fn) { Object.keys(this._params).forEach(k => fn(this._params[k], k, this)); }
    [Symbol.iterator]() {
        var keys = Object.keys(this._params), i = 0, self = this;
        return { next: function() {
            if (i < keys.length) { var k = keys[i++]; return { value: [k, self._params[k]], done: false }; }
            return { done: true };
        }};
    }
    get size() { return Object.keys(this._params).length; }
}
window.URLSearchParams = URLSearchParams;

// -- FormData stub -----------------------------------------------------------
class FormData {
    constructor(form) {
        this._data = {};
    }
    append(name, value, filename) { this._data[name] = value; }
    get(name) { return this._data[name] !== undefined ? this._data[name] : null; }
    has(name) { return name in this._data; }
    set(name, value) { this._data[name] = value; }
    delete(name) { delete this._data[name]; }
    forEach(fn) { Object.keys(this._data).forEach(k => fn(this._data[k], k, this)); }
}
window.FormData = FormData;

// -- Blob / File stubs -------------------------------------------------------
class Blob {
    constructor(parts, options) {
        this.type = (options && options.type) || '';
        this.size = (parts || []).reduce(function(acc, p) { return acc + (p ? (p.length || 0) : 0); }, 0);
    }
    text() { return Promise.resolve(''); }
    arrayBuffer() { return Promise.resolve(new ArrayBuffer(0)); }
    slice() { return new Blob(); }
}
window.Blob = Blob;

class File extends Blob {
    constructor(parts, name, options) {
        super(parts, options);
        this.name = name || '';
        this.lastModified = Date.now();
    }
}
window.File = File;

// -- AbortController stub ----------------------------------------------------
class AbortController {
    constructor() {
        this.signal = { aborted: false, addEventListener: function() {}, removeEventListener: function() {} };
    }
    abort() { this.signal.aborted = true; }
}
window.AbortController = AbortController;
window.AbortSignal = { timeout: function() { return new AbortController().signal; } };

// -- Expose event classes on window ------------------------------------------
window.Event = Event;
window.CustomEvent = CustomEvent;
window.FocusEvent = FocusEvent;
window.InputEvent = InputEvent;
window.MouseEvent = MouseEvent;
window.KeyboardEvent = KeyboardEvent;
window.TouchEvent = TouchEvent;
window.PointerEvent = PointerEvent;
window.WheelEvent = WheelEvent;
window.UIEvent = UIEvent;
window.ErrorEvent = ErrorEvent;
window.MessageEvent = MessageEvent;
window.ProgressEvent = ProgressEvent;
window.StorageEvent = StorageEvent;
window.HashChangeEvent = HashChangeEvent;
window.PopStateEvent = PopStateEvent;
window.BeforeUnloadEvent = BeforeUnloadEvent;
window.MutationObserver = MutationObserver;
window.IntersectionObserver = IntersectionObserver;
window.ResizeObserver = ResizeObserver;
window.Node = Node;
window.Element = Element;
window.Text = TextNode;
window.Comment = Comment;
window.DocumentType = DocumentType;
window.DocumentFragment = DocumentFragment;
window.NodeList = NodeList;
window.HTMLCollection = HTMLCollection;
window.EventTarget = EventTarget;
window.XMLHttpRequest = XMLHttpRequest;
window.DOMTokenList = DOMTokenList;

// -- Image constructor (HTMLImageElement) ------------------------------------
class HTMLImageElement extends Element {
    constructor(width, height) {
        // Create a real img element in the DOM
        var nativeId = __aura_create_element('img');
        super(nativeId, 'img', '');
        if (width !== undefined) __aura_set_attribute(nativeId, 'width', String(width));
        if (height !== undefined) __aura_set_attribute(nativeId, 'height', String(height));
        this.onload = null;
        this.onerror = null;
    }
    get src() { return __aura_get_attribute(this._id, 'src') || ''; }
    set src(v) {
        __aura_set_attribute(this._id, 'src', String(v));
        // Fire onload asynchronously since we can't actually load images
        var self = this;
        setTimeout(function() {
            if (typeof self.onload === 'function') self.onload(new Event('load'));
        }, 0);
    }
    get alt() { return __aura_get_attribute(this._id, 'alt') || ''; }
    set alt(v) { __aura_set_attribute(this._id, 'alt', String(v)); }
    get complete() { return true; }
    get naturalWidth() { return 0; }
    get naturalHeight() { return 0; }
}
// Alias Image to HTMLImageElement (browsers expose Image constructor)
var Image = HTMLImageElement;
window.Image = HTMLImageElement;
window.HTMLImageElement = HTMLImageElement;

// -- Other HTML element constructors -----------------------------------------
class HTMLElement extends Element {
    constructor(id, tag, string_id) {
        super(id, tag, string_id);
    }
}
window.HTMLElement = HTMLElement;

class HTMLDivElement extends HTMLElement {}
window.HTMLDivElement = HTMLDivElement;

class HTMLSpanElement extends HTMLElement {}
window.HTMLSpanElement = HTMLSpanElement;

class HTMLInputElement extends HTMLElement {
    constructor(id, tag, string_id) {
        super(id, tag, string_id);
    }
}
window.HTMLInputElement = HTMLInputElement;

class HTMLButtonElement extends HTMLElement {}
window.HTMLButtonElement = HTMLButtonElement;

class HTMLAnchorElement extends HTMLElement {
    get href() { return __aura_get_attribute(this._id, 'href') || ''; }
    set href(v) { __aura_set_attribute(this._id, 'href', String(v)); }
}
window.HTMLAnchorElement = HTMLAnchorElement;

class HTMLScriptElement extends HTMLElement {
    get src() { return __aura_get_attribute(this._id, 'src') || ''; }
    set src(v) { __aura_set_attribute(this._id, 'src', String(v)); }
    get async() { return __aura_has_attribute(this._id, 'async'); }
    set async(v) { if (v) __aura_set_attribute(this._id, 'async', ''); else __aura_remove_attribute(this._id, 'async'); }
    get defer() { return __aura_has_attribute(this._id, 'defer'); }
}
window.HTMLScriptElement = HTMLScriptElement;

class HTMLStyleElement extends HTMLElement {}
window.HTMLStyleElement = HTMLStyleElement;

class HTMLFormElement extends HTMLElement {
    submit() {}
    reset() {}
}
window.HTMLFormElement = HTMLFormElement;

class HTMLSelectElement extends HTMLElement {
    get value() { return ''; }
    set value(v) {}
    get selectedIndex() { return -1; }
    set selectedIndex(v) {}
    get options() { return new NodeList([]); }
    add(option) {}
    remove(index) {}
}
window.HTMLSelectElement = HTMLSelectElement;

class HTMLOptionElement extends HTMLElement {
    constructor(text, value, defaultSelected, selected) {
        var nativeId = __aura_create_element('option');
        super(nativeId, 'option', '');
        if (text !== undefined) this.textContent = String(text);
        if (value !== undefined) __aura_set_attribute(nativeId, 'value', String(value));
    }
    get value() { return __aura_get_attribute(this._id, 'value') || ''; }
    set value(v) { __aura_set_attribute(this._id, 'value', String(v)); }
    get text() { return this.textContent; }
    set text(v) { this.textContent = String(v); }
    get selected() { return false; }
    set selected(v) {}
    get defaultSelected() { return false; }
}
window.HTMLOptionElement = HTMLOptionElement;
var Option = HTMLOptionElement;
window.Option = HTMLOptionElement;

class HTMLTextAreaElement extends HTMLElement {
    get value() { return __aura_get_text_content(this._id); }
    set value(v) { __aura_set_text_content(this._id, String(v)); }
}
window.HTMLTextAreaElement = HTMLTextAreaElement;

class HTMLLabelElement extends HTMLElement {}
window.HTMLLabelElement = HTMLLabelElement;

class HTMLParagraphElement extends HTMLElement {}
window.HTMLParagraphElement = HTMLParagraphElement;

class HTMLHeadingElement extends HTMLElement {}
window.HTMLHeadingElement = HTMLHeadingElement;

class HTMLTableElement extends HTMLElement {}
window.HTMLTableElement = HTMLTableElement;

class HTMLTableRowElement extends HTMLElement {}
window.HTMLTableRowElement = HTMLTableRowElement;

class HTMLTableCellElement extends HTMLElement {}
window.HTMLTableCellElement = HTMLTableCellElement;

class HTMLUListElement extends HTMLElement {}
window.HTMLUListElement = HTMLUListElement;

class HTMLOListElement extends HTMLElement {}
window.HTMLOListElement = HTMLOListElement;

class HTMLLIElement extends HTMLElement {}
window.HTMLLIElement = HTMLLIElement;

class HTMLCanvasElement extends HTMLElement {
    getContext(type) {
        // Stub canvas context
        return {
            fillRect: function() {},
            clearRect: function() {},
            strokeRect: function() {},
            fillText: function() {},
            strokeText: function() {},
            measureText: function(text) { return { width: text.length * 8 }; },
            drawImage: function() {},
            getImageData: function(x, y, w, h) { return { data: new Uint8ClampedArray(w * h * 4), width: w, height: h }; },
            putImageData: function() {},
            createImageData: function(w, h) { return { data: new Uint8ClampedArray(w * h * 4), width: w, height: h }; },
            save: function() {},
            restore: function() {},
            translate: function() {},
            scale: function() {},
            rotate: function() {},
            transform: function() {},
            setTransform: function() {},
            resetTransform: function() {},
            beginPath: function() {},
            closePath: function() {},
            moveTo: function() {},
            lineTo: function() {},
            quadraticCurveTo: function() {},
            bezierCurveTo: function() {},
            arc: function() {},
            arcTo: function() {},
            ellipse: function() {},
            rect: function() {},
            fill: function() {},
            stroke: function() {},
            clip: function() {},
            isPointInPath: function() { return false; },
            createLinearGradient: function() { return { addColorStop: function() {} }; },
            createRadialGradient: function() { return { addColorStop: function() {} }; },
            createPattern: function() { return null; },
            addEventListener: function() {},
            canvas: null,
            fillStyle: '',
            strokeStyle: '',
            lineWidth: 1,
            font: '10px sans-serif',
            textAlign: 'start',
            textBaseline: 'alphabetic',
            globalAlpha: 1,
            globalCompositeOperation: 'source-over',
        };
    }
    get width() { return parseInt(__aura_get_attribute(this._id, 'width') || '300'); }
    set width(v) { __aura_set_attribute(this._id, 'width', String(v)); }
    get height() { return parseInt(__aura_get_attribute(this._id, 'height') || '150'); }
    set height(v) { __aura_set_attribute(this._id, 'height', String(v)); }
    toDataURL(type, quality) { return 'data:image/png;base64,'; }
    toBlob(callback) { callback(new Blob()); }
}
window.HTMLCanvasElement = HTMLCanvasElement;

// -- window.queueMicrotask ---------------------------------------------------
window.queueMicrotask = function(fn) {
    if (typeof fn === 'function') {
        Promise.resolve().then(fn);
    }
};

// -- atob / btoa stubs -------------------------------------------------------
window.atob = function(s) { return ''; };
window.btoa = function(s) { return ''; };

// -- window.scroll / scrollTo / scrollBy ------------------------------------
window.scroll = function() {};
window.scrollTo = function() {};
window.scrollBy = function() {};

// -- window.requestIdleCallback already set above via native ----------------
window.cancelAnimationFrame = function() {};

// -- Expose console on window ------------------------------------------------
window.console = console;

// -- Intl stub ---------------------------------------------------------------
if (typeof Intl === 'undefined') {
    window.Intl = {
        DateTimeFormat: function() { return { format: function(d) { return d ? d.toString() : ''; } }; },
        NumberFormat: function() { return { format: function(n) { return String(n); } }; },
        Collator: function() { return { compare: function(a, b) { return a < b ? -1 : a > b ? 1 : 0; } }; },
    };
}
