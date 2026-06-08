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
    if (loc && typeof loc._setHref === 'function') {
        loc._setHref(parts.href, loc.href);
        window.location = loc;
        location = loc;
        return true;
    }
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

function __aura_resolve_url_attribute(value) {
    var raw = String(value || '');
    if (!raw) return '';
    if (typeof __aura_resolve_url === 'function') {
        return __aura_resolve_url(raw, location.href || document.baseURI || '');
    }
    try {
        return new URL(raw, location.href || document.baseURI || undefined).href;
    } catch (e) {
        return raw;
    }
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
        let tagLower = descriptor.tag.toLowerCase();
        if (tagLower === 'iframe') {
            node = new HTMLIFrameElement(id, descriptor.tag, descriptor.id);
        } else if (tagLower === 'form') {
            node = new HTMLFormElement(id, descriptor.tag, descriptor.id);
        } else if (tagLower === 'a') {
            node = new HTMLAnchorElement(id, descriptor.tag, descriptor.id);
        } else if (tagLower === 'script') {
            node = new HTMLScriptElement(id, descriptor.tag, descriptor.id);
        } else if (tagLower === 'img') {
            node = new HTMLImageElement(id, descriptor.tag, descriptor.id);
        } else if (tagLower === 'input') {
            node = new HTMLInputElement(id, descriptor.tag, descriptor.id);
        } else if (tagLower === 'button') {
            node = new HTMLButtonElement(id, descriptor.tag, descriptor.id);
        } else if (tagLower === 'div') {
            node = new HTMLDivElement(id, descriptor.tag, descriptor.id);
        } else if (tagLower === 'span') {
            node = new HTMLSpanElement(id, descriptor.tag, descriptor.id);
        } else if (tagLower === 'style') {
            node = new HTMLStyleElement(id, descriptor.tag, descriptor.id);
        } else if (tagLower === 'canvas') {
            node = new HTMLCanvasElement(id, descriptor.tag, descriptor.id);
        } else {
            node = new Element(id, descriptor.tag, descriptor.id);
        }
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

var NodeFilter = {
    FILTER_ACCEPT: 1,
    FILTER_REJECT: 2,
    FILTER_SKIP: 3,
    SHOW_ALL: 0xFFFFFFFF,
    SHOW_ELEMENT: 0x1,
    SHOW_ATTRIBUTE: 0x2,
    SHOW_TEXT: 0x4,
    SHOW_CDATA_SECTION: 0x8,
    SHOW_ENTITY_REFERENCE: 0x10,
    SHOW_ENTITY: 0x20,
    SHOW_PROCESSING_INSTRUCTION: 0x40,
    SHOW_COMMENT: 0x80,
    SHOW_DOCUMENT: 0x100,
    SHOW_DOCUMENT_TYPE: 0x200,
    SHOW_DOCUMENT_FRAGMENT: 0x400
};

function __aura_what_to_show_mask(node) {
    if (!node || !node.nodeType) return 0;
    return 1 << (node.nodeType - 1);
}

function __aura_filter_result(node, whatToShow, filter) {
    let mask = whatToShow === undefined || whatToShow === null ? NodeFilter.SHOW_ALL : whatToShow;
    if ((mask & __aura_what_to_show_mask(node)) === 0) return NodeFilter.FILTER_SKIP;
    if (!filter) return NodeFilter.FILTER_ACCEPT;
    let result;
    if (typeof filter === 'function') {
        result = filter(node);
    } else if (typeof filter.acceptNode === 'function') {
        result = filter.acceptNode(node);
    }
    return result || NodeFilter.FILTER_ACCEPT;
}

function __aura_tree_order(root) {
    let nodes = [];
    function walk(node) {
        if (!node) return;
        nodes.push(node);
        let children = node.childNodes || [];
        for (let i = 0; i < children.length; i++) walk(children[i]);
    }
    walk(root);
    return nodes;
}

class TreeWalker {
    constructor(root, whatToShow, filter) {
        this.root = root;
        this.whatToShow = whatToShow === undefined || whatToShow === null ? NodeFilter.SHOW_ALL : whatToShow;
        this.filter = filter || null;
        this.currentNode = root;
    }
    _visible(node) {
        return __aura_filter_result(node, this.whatToShow, this.filter) === NodeFilter.FILTER_ACCEPT;
    }
    _ordered() {
        return __aura_tree_order(this.root);
    }
    nextNode() {
        let nodes = this._ordered();
        let start = nodes.indexOf(this.currentNode);
        for (let i = start + 1; i < nodes.length; i++) {
            if (this._visible(nodes[i])) {
                this.currentNode = nodes[i];
                return nodes[i];
            }
        }
        return null;
    }
    previousNode() {
        let nodes = this._ordered();
        let start = nodes.indexOf(this.currentNode);
        if (start < 0) start = nodes.length;
        for (let i = start - 1; i >= 0; i--) {
            if (this._visible(nodes[i])) {
                this.currentNode = nodes[i];
                return nodes[i];
            }
        }
        return null;
    }
    parentNode() {
        let node = this.currentNode.parentNode;
        while (node) {
            if (node === this.root || this._ordered().includes(node)) {
                if (this._visible(node)) {
                    this.currentNode = node;
                    return node;
                }
            }
            if (node === this.root) break;
            node = node.parentNode;
        }
        return null;
    }
    firstChild() {
        let nodes = this.currentNode.childNodes || [];
        for (let i = 0; i < nodes.length; i++) {
            if (this._visible(nodes[i])) {
                this.currentNode = nodes[i];
                return nodes[i];
            }
        }
        return null;
    }
    lastChild() {
        let nodes = this.currentNode.childNodes || [];
        for (let i = nodes.length - 1; i >= 0; i--) {
            if (this._visible(nodes[i])) {
                this.currentNode = nodes[i];
                return nodes[i];
            }
        }
        return null;
    }
    nextSibling() {
        let parent = this.currentNode.parentNode;
        if (!parent) return null;
        let siblings = parent.childNodes || [];
        let start = Array.from(siblings).indexOf(this.currentNode);
        for (let i = start + 1; i < siblings.length; i++) {
            if (this._visible(siblings[i])) {
                this.currentNode = siblings[i];
                return siblings[i];
            }
        }
        return null;
    }
    previousSibling() {
        let parent = this.currentNode.parentNode;
        if (!parent) return null;
        let siblings = parent.childNodes || [];
        let start = Array.from(siblings).indexOf(this.currentNode);
        for (let i = start - 1; i >= 0; i--) {
            if (this._visible(siblings[i])) {
                this.currentNode = siblings[i];
                return siblings[i];
            }
        }
        return null;
    }
}

class NodeIterator {
    constructor(root, whatToShow, filter) {
        this.root = root;
        this.whatToShow = whatToShow === undefined || whatToShow === null ? NodeFilter.SHOW_ALL : whatToShow;
        this.filter = filter || null;
        this.referenceNode = root;
        this.pointerBeforeReferenceNode = true;
        this._detached = false;
    }
    _visible(node) {
        return __aura_filter_result(node, this.whatToShow, this.filter) === NodeFilter.FILTER_ACCEPT;
    }
    _visibleNodes() {
        return __aura_tree_order(this.root).filter(node => this._visible(node));
    }
    nextNode() {
        if (this._detached) return null;
        let nodes = this._visibleNodes();
        let index = nodes.indexOf(this.referenceNode);
        if (this.pointerBeforeReferenceNode && index >= 0) {
            this.pointerBeforeReferenceNode = false;
            return this.referenceNode;
        }
        let next = nodes[index + 1];
        if (!next) return null;
        this.referenceNode = next;
        this.pointerBeforeReferenceNode = false;
        return next;
    }
    previousNode() {
        if (this._detached) return null;
        let nodes = this._visibleNodes();
        let index = nodes.indexOf(this.referenceNode);
        if (!this.pointerBeforeReferenceNode && index >= 0) {
            this.pointerBeforeReferenceNode = true;
            return this.referenceNode;
        }
        let previous = nodes[index - 1];
        if (!previous) return null;
        this.referenceNode = previous;
        this.pointerBeforeReferenceNode = true;
        return previous;
    }
    detach() {
        this._detached = true;
    }
}

function __aura_child_at(container, offset) {
    if (!container || !container.childNodes) return null;
    return container.childNodes.item(offset);
}

function __aura_node_index(node) {
    if (!node || !node.parentNode) return -1;
    let siblings = node.parentNode.childNodes;
    for (let i = 0; i < siblings.length; i++) {
        if (siblings[i] === node) return i;
    }
    return -1;
}

function __aura_is_character_data(node) {
    return node && (node.nodeType === Node.TEXT_NODE || node.nodeType === Node.COMMENT_NODE);
}

function __aura_common_ancestor(a, b) {
    let ancestors = [];
    let current = a;
    while (current) {
        ancestors.push(current);
        current = current.parentNode;
    }
    current = b;
    while (current) {
        if (ancestors.includes(current)) return current;
        current = current.parentNode;
    }
    return null;
}

var __aura_mutation_observers = [];
var __aura_mutation_delivery_scheduled = false;

function __aura_static_node_list(nodes) {
    return new NodeList((nodes || []).map(node => node && node._id).filter(id => id !== undefined && id !== null));
}

function __aura_mutation_options_match(record, options) {
    if (record.type === 'childList') return !!options.childList;
    if (record.type === 'attributes') {
        if (!options.attributes) return false;
        if (options.attributeFilter && !options.attributeFilter.includes(record.attributeName)) return false;
        return true;
    }
    if (record.type === 'characterData') return !!options.characterData;
    return false;
}

function __aura_observes_target(observedTarget, recordTarget, subtree) {
    if (observedTarget === recordTarget) return true;
    return !!subtree && observedTarget && observedTarget.contains && observedTarget.contains(recordTarget);
}

function __aura_schedule_mutation_delivery() {
    if (__aura_mutation_delivery_scheduled) return;
    __aura_mutation_delivery_scheduled = true;
    Promise.resolve().then(function() {
        __aura_mutation_delivery_scheduled = false;
        for (let observer of __aura_mutation_observers.slice()) {
            let records = observer.takeRecords();
            if (records.length > 0) observer._callback(records, observer);
        }
    });
}

function __aura_queue_mutation(record) {
    let queuedAny = false;
    for (let observer of __aura_mutation_observers) {
        for (let registration of observer._registrations) {
            if (!__aura_observes_target(registration.target, record.target, registration.options.subtree)) continue;
            if (!__aura_mutation_options_match(record, registration.options)) continue;
            let queued = Object.assign({}, record);
            if (queued.type === 'attributes' && !registration.options.attributeOldValue) queued.oldValue = null;
            if (queued.type === 'characterData' && !registration.options.characterDataOldValue) queued.oldValue = null;
            observer._records.push(queued);
            queuedAny = true;
            break;
        }
    }
    if (queuedAny) __aura_schedule_mutation_delivery();
}

function __aura_child_list_mutation(target, addedNodes, removedNodes, previousSibling, nextSibling) {
    __aura_queue_mutation({
        type: 'childList',
        target,
        addedNodes: __aura_static_node_list(addedNodes),
        removedNodes: __aura_static_node_list(removedNodes),
        previousSibling: previousSibling || null,
        nextSibling: nextSibling || null,
        attributeName: null,
        attributeNamespace: null,
        oldValue: null
    });
}

function __aura_attribute_mutation(target, name, oldValue) {
    __aura_queue_mutation({
        type: 'attributes',
        target,
        addedNodes: new NodeList([]),
        removedNodes: new NodeList([]),
        previousSibling: null,
        nextSibling: null,
        attributeName: String(name),
        attributeNamespace: null,
        oldValue
    });
}

function __aura_character_data_mutation(target, oldValue) {
    __aura_queue_mutation({
        type: 'characterData',
        target,
        addedNodes: new NodeList([]),
        removedNodes: new NodeList([]),
        previousSibling: null,
        nextSibling: null,
        attributeName: null,
        attributeNamespace: null,
        oldValue
    });
}

class Range {
    constructor() {
        this.startContainer = document;
        this.startOffset = 0;
        this.endContainer = document;
        this.endOffset = 0;
    }
    setStart(node, offset) {
        this.startContainer = node;
        this.startOffset = Math.max(0, Number(offset) || 0);
    }
    setEnd(node, offset) {
        this.endContainer = node;
        this.endOffset = Math.max(0, Number(offset) || 0);
    }
    setStartBefore(node) {
        this.setStart(node.parentNode, __aura_node_index(node));
    }
    setStartAfter(node) {
        this.setStart(node.parentNode, __aura_node_index(node) + 1);
    }
    setEndBefore(node) {
        this.setEnd(node.parentNode, __aura_node_index(node));
    }
    setEndAfter(node) {
        this.setEnd(node.parentNode, __aura_node_index(node) + 1);
    }
    selectNode(node) {
        let index = __aura_node_index(node);
        this.setStart(node.parentNode, index);
        this.setEnd(node.parentNode, index + 1);
    }
    selectNodeContents(node) {
        this.setStart(node, 0);
        if (__aura_is_character_data(node)) {
            this.setEnd(node, node.data.length);
        } else {
            this.setEnd(node, node.childNodes ? node.childNodes.length : 0);
        }
    }
    collapse(toStart) {
        if (toStart === false) {
            this.setStart(this.endContainer, this.endOffset);
        } else {
            this.setEnd(this.startContainer, this.startOffset);
        }
    }
    cloneRange() {
        let range = new Range();
        range.setStart(this.startContainer, this.startOffset);
        range.setEnd(this.endContainer, this.endOffset);
        return range;
    }
    get collapsed() {
        return this.startContainer === this.endContainer && this.startOffset === this.endOffset;
    }
    get commonAncestorContainer() {
        return __aura_common_ancestor(this.startContainer, this.endContainer);
    }
    _selectedChildren() {
        if (this.startContainer !== this.endContainer || __aura_is_character_data(this.startContainer)) return [];
        let nodes = [];
        let children = this.startContainer.childNodes || new NodeList([]);
        let end = Math.min(this.endOffset, children.length);
        for (let i = this.startOffset; i < end; i++) {
            let child = children.item(i);
            if (child) nodes.push(child);
        }
        return nodes;
    }
    cloneContents() {
        let fragment = document.createDocumentFragment();
        if (this.startContainer === this.endContainer && __aura_is_character_data(this.startContainer)) {
            fragment.appendChild(document.createTextNode(this.startContainer.data.slice(this.startOffset, this.endOffset)));
            return fragment;
        }
        let nodes = this._selectedChildren();
        for (let i = 0; i < nodes.length; i++) {
            fragment.appendChild(nodes[i].cloneNode(true));
        }
        return fragment;
    }
    extractContents() {
        let fragment = document.createDocumentFragment();
        if (this.startContainer === this.endContainer && __aura_is_character_data(this.startContainer)) {
            let node = this.startContainer;
            let data = node.data;
            fragment.appendChild(document.createTextNode(data.slice(this.startOffset, this.endOffset)));
            node.data = data.slice(0, this.startOffset) + data.slice(this.endOffset);
            this.collapse(true);
            return fragment;
        }
        let nodes = this._selectedChildren();
        for (let i = 0; i < nodes.length; i++) {
            fragment.appendChild(nodes[i]);
        }
        this.collapse(true);
        return fragment;
    }
    deleteContents() {
        this.extractContents();
    }
    insertNode(node) {
        if (__aura_is_character_data(this.startContainer)) {
            let text = this.startContainer;
            let parent = text.parentNode;
            let after = document.createTextNode(text.data.slice(this.startOffset));
            text.data = text.data.slice(0, this.startOffset);
            parent.insertBefore(after, text.nextSibling);
            parent.insertBefore(node, after);
            return;
        }
        this.startContainer.insertBefore(node, __aura_child_at(this.startContainer, this.startOffset));
    }
    surroundContents(newParent) {
        let fragment = this.extractContents();
        newParent.appendChild(fragment);
        this.insertNode(newParent);
        this.selectNode(newParent);
    }
    toString() {
        if (this.startContainer === this.endContainer && __aura_is_character_data(this.startContainer)) {
            return this.startContainer.data.slice(this.startOffset, this.endOffset);
        }
        let nodes = this._selectedChildren();
        if (nodes.length > 0) {
            return nodes.map(node => node.textContent || '').join('');
        }
        return '';
    }
    getBoundingClientRect() {
        return { x:0, y:0, width:0, height:0, top:0, left:0, right:0, bottom:0 };
    }
    getClientRects() {
        return [];
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
    getAttribute(name) { return null; }
    setAttribute(name, value) {}
    removeAttribute(name) {}
    hasAttribute(name) { return false; }
    closest(selector) { return null; }
    matches(selector) { return false; }
    querySelector(selector) { return null; }
    querySelectorAll(selector) { return new NodeList([]); }
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
        let oldChildren = Array.from(this.childNodes || []);
        __aura_set_text_content(this._id, String(val));
        __aura_child_list_mutation(this, Array.from(this.childNodes || []), oldChildren, null, null);
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
            let added = child.nodeType === Node.DOCUMENT_FRAGMENT_NODE ? Array.from(child.childNodes) : [child];
            let oldParent = child.nodeType === Node.DOCUMENT_FRAGMENT_NODE ? null : child.parentNode;
            let oldPrevious = child.previousSibling;
            let oldNext = child.nextSibling;
            let previous = this.lastChild;
            __aura_append_child(this._id, child._id);
            if (oldParent && oldParent !== this) __aura_child_list_mutation(oldParent, [], [child], oldPrevious, oldNext);
            if (added.length > 0) __aura_child_list_mutation(this, added, [], previous, null);
            added.forEach(__aura_maybe_run_script);
        }
        return child;
    }
    removeChild(child) {
        if (child && child._id !== undefined && child._id !== null) {
            let previous = child.previousSibling;
            let next = child.nextSibling;
            __aura_remove_child(this._id, child._id);
            __aura_child_list_mutation(this, [], [child], previous, next);
        }
        return child;
    }
    insertBefore(newChild, refChild) {
        if (newChild && newChild._id !== undefined && newChild._id !== null) {
            let added = newChild.nodeType === Node.DOCUMENT_FRAGMENT_NODE ? Array.from(newChild.childNodes) : [newChild];
            let oldParent = newChild.nodeType === Node.DOCUMENT_FRAGMENT_NODE ? null : newChild.parentNode;
            let oldPrevious = newChild.previousSibling;
            let oldNext = newChild.nextSibling;
            let previous = refChild ? refChild.previousSibling : this.lastChild;
            let next = refChild || null;
            __aura_insert_before(this._id, newChild._id, refChild ? refChild._id : null);
            if (oldParent && oldParent !== this) __aura_child_list_mutation(oldParent, [], [newChild], oldPrevious, oldNext);
            if (added.length > 0) __aura_child_list_mutation(this, added, [], previous, next);
            added.forEach(__aura_maybe_run_script);
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
    get ownerDocument() {
        return document;
    }
    getRootNode(options) {
        return this.ownerDocument || document;
    }
}

// -- CSSOM -------------------------------------------------------------------
function __aura_css_camel_to_kebab(name) {
    return String(name).replace(/([A-Z])/g, "-$1").toLowerCase();
}

function __aura_css_kebab_to_camel(name) {
    return String(name).replace(/-([a-z])/g, function(_, ch) { return ch.toUpperCase(); });
}

class CSSStyleDeclaration {
    constructor(ownerId = null, readonly = false, initial = null) {
        this._ownerId = ownerId;
        this._readonly = readonly;
        this._props = [];
        if (initial) {
            Object.keys(initial).forEach(name => this.setProperty(name, initial[name]));
        }
        return new Proxy(this, {
            get(target, prop, receiver) {
                if (prop === 'length') return target._props.length;
                if (typeof prop === 'string' && /^(0|[1-9]\d*)$/.test(prop)) return target.item(Number(prop));
                if (typeof prop === 'string' && !(prop in target)) return target.getPropertyValue(__aura_css_camel_to_kebab(prop));
                let value = Reflect.get(target, prop, receiver);
                return typeof value === 'function' ? value.bind(target) : value;
            },
            set(target, prop, value, receiver) {
                if (typeof prop === 'string' && !(prop in target) && prop[0] !== '_') {
                    target.setProperty(__aura_css_camel_to_kebab(prop), value);
                    return true;
                }
                return Reflect.set(target, prop, value, receiver);
            }
        });
    }
    _find(name) {
        name = String(name).toLowerCase();
        return this._props.find(entry => entry.name === name);
    }
    _assertWritable() {
        if (this._readonly) throw new Error('CSSStyleDeclaration is read-only');
    }
    get cssText() {
        return this._props.map(entry => entry.name + ': ' + entry.value + (entry.priority ? ' !' + entry.priority : '') + ';').join(' ');
    }
    set cssText(value) {
        this._assertWritable();
        this._props = [];
        String(value || '').split(';').forEach(part => {
            let idx = part.indexOf(':');
            if (idx <= 0) return;
            let name = part.slice(0, idx).trim();
            let rawValue = part.slice(idx + 1).trim();
            let priority = '';
            if (/!\s*important$/i.test(rawValue)) {
                rawValue = rawValue.replace(/!\s*important$/i, '').trim();
                priority = 'important';
            }
            this.setProperty(name, rawValue, priority);
        });
    }
    getPropertyValue(name) {
        let entry = this._find(name);
        return entry ? entry.value : '';
    }
    getPropertyPriority(name) {
        let entry = this._find(name);
        return entry ? entry.priority : '';
    }
    setProperty(name, value, priority = '') {
        this._assertWritable();
        name = String(name).trim().toLowerCase();
        if (!name) return;
        value = String(value == null ? '' : value).trim();
        priority = String(priority || '').toLowerCase();
        if (priority && priority !== 'important') return;
        let entry = this._find(name);
        if (!entry) {
            entry = { name, value: '', priority: '' };
            this._props.push(entry);
        }
        entry.value = value;
        entry.priority = priority;
        this[__aura_css_kebab_to_camel(name)] = value;
        if (this._ownerId !== null) __aura_set_style(this._ownerId, name, value);
    }
    removeProperty(name) {
        this._assertWritable();
        name = String(name).trim().toLowerCase();
        let old = this.getPropertyValue(name);
        this._props = this._props.filter(entry => entry.name !== name);
        if (this._ownerId !== null) __aura_set_style(this._ownerId, name, '');
        return old;
    }
    item(index) {
        let entry = this._props[index];
        return entry ? entry.name : '';
    }
}

class CSSStyleRule {
    constructor(selectorText, body) {
        this.type = CSSRule.STYLE_RULE;
        this.selectorText = String(selectorText || '').trim();
        this.style = new CSSStyleDeclaration();
        this.style.cssText = body || '';
    }
    get cssText() {
        return this.selectorText + ' { ' + this.style.cssText + ' }';
    }
}

class CSSStyleSheet {
    constructor(text = '') {
        this.disabled = false;
        this.href = null;
        this.ownerNode = null;
        this.parentStyleSheet = null;
        this.title = null;
        this.type = 'text/css';
        this.cssRules = [];
        this.replaceSync(text);
    }
    _parse(text) {
        let rules = [];
        let re = /([^{}]+)\{([^{}]*)\}/g;
        let match;
        while ((match = re.exec(String(text || ''))) !== null) {
            rules.push(new CSSStyleRule(match[1], match[2]));
        }
        return rules;
    }
    insertRule(rule, index = this.cssRules.length) {
        let parsed = this._parse(rule);
        if (parsed.length === 0) throw new Error('Invalid CSS rule');
        index = Number(index);
        if (index < 0 || index > this.cssRules.length) throw new Error('IndexSizeError');
        this.cssRules.splice(index, 0, parsed[0]);
        return index;
    }
    deleteRule(index) {
        index = Number(index);
        if (index < 0 || index >= this.cssRules.length) throw new Error('IndexSizeError');
        this.cssRules.splice(index, 1);
    }
    replaceSync(text) {
        this.cssRules = this._parse(text);
    }
    replace(text) {
        this.replaceSync(text);
        return Promise.resolve(this);
    }
    get rules() { return this.cssRules; }
}

class StyleSheetList {
    constructor(resolver) {
        this._resolver = resolver;
        return new Proxy(this, {
            get(target, prop, receiver) {
                if (prop === 'length') return target._items().length;
                if (typeof prop === 'string' && /^(0|[1-9]\d*)$/.test(prop)) return target.item(Number(prop));
                let value = Reflect.get(target, prop, receiver);
                return typeof value === 'function' ? value.bind(target) : value;
            }
        });
    }
    _items() { return this._resolver(); }
    item(index) { return this._items()[index] || null; }
    [Symbol.iterator]() {
        let i = 0, self = this;
        return { next() { return i < self.length ? { value: self.item(i++), done: false } : { done: true }; } };
    }
}

var CSSRule = { STYLE_RULE: 1, IMPORT_RULE: 3, MEDIA_RULE: 4, FONT_FACE_RULE: 5, PAGE_RULE: 6 };
var CSS = {
    supports: function(property, value) {
        if (value === undefined) return String(property).indexOf(':') > 0;
        return String(property).trim() !== '' && String(value).trim() !== '';
    },
    escape: function(value) {
        return String(value).replace(/[^a-zA-Z0-9_-]/g, function(ch) {
            return '\\' + ch.charCodeAt(0).toString(16) + ' ';
        });
    }
};

class Attr {
    constructor(element, name, value) {
        this.name = name;
        this.value = value;
        this.ownerElement = element;
    }
}

class NamedNodeMap {
    constructor(element) {
        this._element = element;
    }
    get length() {
        let attrs = typeof __aura_get_attributes === 'function'
            ? JSON.parse(__aura_get_attributes(this._element._id))
            : [];
        return attrs.length;
    }
    item(index) {
        let attrs = typeof __aura_get_attributes === 'function'
            ? JSON.parse(__aura_get_attributes(this._element._id))
            : [];
        if (index >= 0 && index < attrs.length) {
            return new Attr(this._element, attrs[index].name, attrs[index].value);
        }
        return null;
    }
    getNamedItem(name) {
        let value = __aura_get_attribute(this._element._id, name);
        if (value !== null) {
            return new Attr(this._element, name, value);
        }
        return null;
    }
    setNamedItem(attr) {
        this._element.setAttribute(attr.name, attr.value);
    }
    removeNamedItem(name) {
        this._element.removeAttribute(name);
    }
}

class Element extends Node {
    constructor(id, tag, string_id) {
        super(id, 'element');
        this.tagName = (tag || '').toUpperCase();
        this.id = string_id || '';
        this._classList = null;
        this.style = new CSSStyleDeclaration(id);
    }
    get classList() {
        if (!this._classList) this._classList = new DOMTokenList(this._id);
        return this._classList;
    }
    get attributes() {
        let map = new NamedNodeMap(this);
        return new Proxy(map, {
            get: function(target, prop) {
                if (typeof prop === 'string' && /^\d+$/.test(prop)) {
                    return target.item(Number(prop));
                }
                if (prop in target) {
                    let value = target[prop];
                    if (typeof value === 'function') {
                        return value.bind(target);
                    }
                    return value;
                }
                if (typeof prop === 'string') {
                    return target.getNamedItem(prop);
                }
                return undefined;
            }
        });
    }
    get className() {
        return __aura_get_attribute(this._id, 'class') || '';
    }
    set className(val) {
        let oldValue = this.getAttribute('class');
        __aura_set_attribute(this._id, 'class', String(val));
        __aura_attribute_mutation(this, 'class', oldValue);
    }
    get innerHTML() {
        return __aura_get_inner_html(this._id);
    }
    set innerHTML(val) {
        let oldChildren = Array.from(this.childNodes || []);
        __aura_set_inner_html(this._id, String(val));
        __aura_child_list_mutation(this, Array.from(this.childNodes || []), oldChildren, null, null);
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
        let oldChildren = Array.from(this.childNodes || []);
        __aura_set_text_content(this._id, String(val));
        __aura_child_list_mutation(this, Array.from(this.childNodes || []), oldChildren, null, null);
    }
    setAttribute(name, value) {
        let oldValue = this.getAttribute(name);
        __aura_set_attribute(this._id, name, String(value));
        __aura_attribute_mutation(this, name, oldValue);
    }
    getAttribute(name) {
        return __aura_get_attribute(this._id, name);
    }
    removeAttribute(name) {
        let oldValue = this.getAttribute(name);
        __aura_remove_attribute(this._id, name);
        __aura_attribute_mutation(this, name, oldValue);
    }
    hasAttribute(name) {
        return __aura_has_attribute(this._id, name);
    }
    remove() {
        let parent = this.parentNode;
        let previous = this.previousSibling;
        let next = this.nextSibling;
        __aura_remove_self(this._id);
        if (parent) __aura_child_list_mutation(parent, [], [this], previous, next);
    }
    focus() {
        __aura_set_focus(this.id);
        document.activeElement = this;
        this.dispatchEvent(new Event('focus', { bubbles: false }));
        this.dispatchEvent(new Event('focusin', { bubbles: true }));
    }
    blur() {
        if (document.activeElement === this) document.activeElement = document.body;
        this.dispatchEvent(new Event('blur', { bubbles: false }));
        this.dispatchEvent(new Event('focusout', { bubbles: true }));
    }
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
    get form() {
        let node = this.parentElement;
        while (node) {
            if (node.tagName === 'FORM') return node;
            node = node.parentElement;
        }
        return null;
    }
    get defaultValue() {
        if (this.tagName === 'TEXTAREA') return __aura_get_attribute(this._id, 'value') || this.textContent || '';
        return __aura_get_attribute(this._id, 'value') || '';
    }
    set defaultValue(v) {
        this.setAttribute('value', String(v));
    }
    get value() {
        if (this.tagName === 'TEXTAREA') return this.textContent || '';
        if (this.tagName === 'SELECT') {
            let option = this.options.item(this.selectedIndex);
            return option ? option.value : '';
        }
        if (this.tagName === 'OPTION') {
            let attr = __aura_get_attribute(this._id, 'value');
            return attr !== null ? attr : this.textContent;
        }
        return __aura_get_attribute(this._id, 'value') || '';
    }
    set value(v) {
        let value = String(v);
        if (this.tagName === 'TEXTAREA') {
            this.textContent = value;
            this.setAttribute('value', value);
            return;
        }
        if (this.tagName === 'SELECT') {
            let options = Array.from(this.options);
            let matched = false;
            for (let i = 0; i < options.length; i++) {
                let selected = !matched && options[i].value === value;
                options[i].selected = selected;
                matched = matched || selected;
            }
            return;
        }
        this.setAttribute('value', value);
    }
    get checked() { return __aura_has_attribute(this._id, 'checked'); }
    set checked(v) {
        let checked = !!v;
        if (checked && this.type === 'radio' && this.name) {
            let root = this.form || document;
            let radios = root.querySelectorAll('input');
            for (let i = 0; i < radios.length; i++) {
                let radio = radios.item(i);
                if (radio !== this && radio.type === 'radio' && radio.name === this.name) {
                    radio.removeAttribute('checked');
                }
            }
        }
        if (checked) this.setAttribute('checked', 'checked');
        else this.removeAttribute('checked');
    }
    get defaultChecked() { return this.checked; }
    set defaultChecked(v) { this.checked = v; }
    get disabled() { return __aura_has_attribute(this._id, 'disabled'); }
    set disabled(v) {
        if (v) this.setAttribute('disabled', 'disabled');
        else this.removeAttribute('disabled');
    }
    get href() { return __aura_resolve_url_attribute(__aura_get_attribute(this._id, 'href') || ''); }
    set href(v) { __aura_set_attribute(this._id, 'href', String(v)); }
    get src() { return __aura_resolve_url_attribute(__aura_get_attribute(this._id, 'src') || ''); }
    set src(v) { __aura_set_attribute(this._id, 'src', String(v)); }
    get type() { return __aura_get_attribute(this._id, 'type') || ''; }
    set type(v) { __aura_set_attribute(this._id, 'type', String(v)); }
    get name() { return __aura_get_attribute(this._id, 'name') || ''; }
    set name(v) { __aura_set_attribute(this._id, 'name', String(v)); }
    get options() {
        if (this.tagName !== 'SELECT') return new NodeList([]);
        return this.querySelectorAll('option');
    }
    get selectedIndex() {
        if (this.tagName !== 'SELECT') return -1;
        let options = Array.from(this.options);
        for (let i = 0; i < options.length; i++) {
            if (options[i].selected) return i;
        }
        return options.length > 0 ? 0 : -1;
    }
    set selectedIndex(v) {
        if (this.tagName !== 'SELECT') return;
        let index = Number(v);
        let options = Array.from(this.options);
        for (let i = 0; i < options.length; i++) {
            options[i].selected = i === index;
        }
    }
    get selected() {
        if (this.tagName !== 'OPTION') return false;
        return __aura_has_attribute(this._id, 'selected');
    }
    set selected(v) {
        if (this.tagName !== 'OPTION') return;
        if (v) {
            let select = this.parentElement;
            while (select && select.tagName !== 'SELECT') select = select.parentElement;
            if (select && !select.multiple) {
                let options = Array.from(select.options);
                for (let i = 0; i < options.length; i++) {
                    if (options[i] !== this) options[i].removeAttribute('selected');
                }
            }
            this.setAttribute('selected', 'selected');
        } else {
            this.removeAttribute('selected');
        }
    }
    get defaultSelected() { return this.selected; }
    set defaultSelected(v) { this.selected = v; }
    get multiple() { return __aura_has_attribute(this._id, 'multiple'); }
    set multiple(v) {
        if (v) this.setAttribute('multiple', 'multiple');
        else this.removeAttribute('multiple');
    }
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
    attachShadow(options) {
        if (this._shadowRoot) throw new Error('Shadow root already attached');
        let mode = options && options.mode === 'closed' ? 'closed' : 'open';
        let fragmentId = __aura_create_document_fragment();
        let root = new ShadowRoot(fragmentId, this, mode);
        __node_registry.set(fragmentId, root);
        this._shadowRoot = root;
        return root;
    }
    get shadowRoot() {
        return this._shadowRoot && this._shadowRoot.mode === 'open' ? this._shadowRoot : null;
    }
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

// -- IDL Event Handler Properties (Element.prototype, document, globalThis) ----
(function() {
    var handlers = [
        'onclick', 'onkeydown', 'onkeyup',
        'onsubmit', 'oninput', 'onchange',
        'onload', 'onerror'
    ];
    var defs = {};
    for (var i = 0; i < handlers.length; i++) {
        (function(name) {
            var key = '_' + name;
            defs[name] = {
                get: function() { return this[key] || null; },
                set: function(fn) { this[key] = fn; }
            };
        })(handlers[i]);
    }
    try {
        Object.defineProperties(Element.prototype, defs);
    } catch (e) {
        console.warn('Failed to define handlers on Element: ' + e);
    }
    try {
        Object.defineProperties(document, defs);
    } catch (e) {
        console.warn('Failed to define handlers on document: ' + e);
    }
    for (var key in defs) {
        try {
            Object.defineProperty(globalThis, key, defs[key]);
        } catch (e) {
            try {
                globalThis[key] = null;
            } catch (err) {}
        }
    }
})();

class CharacterData extends Node {
    constructor(id, kind) {
        super(id, kind);
        this._data = '';
    }
    get data() {
        return this._id ? __aura_read_character_data(this._id, this._kind) : this._data;
    }
    set data(val) {
        let text = String(val);
        let oldValue = this.data;
        if (!__aura_write_character_data(this._id, this._kind, text)) {
            this._data = text;
        }
        __aura_character_data_mutation(this, oldValue);
    }
    get length() {
        return this.data.length;
    }
    appendData(text) {
        this.data = this.data + String(text);
    }
    deleteData(offset, count) {
        let d = this.data;
        offset = Math.min(d.length, Math.max(0, Number(offset) || 0));
        count = Math.max(0, Number(count) || 0);
        this.data = d.slice(0, offset) + d.slice(offset + count);
    }
    insertData(offset, text) {
        let d = this.data;
        offset = Math.min(d.length, Math.max(0, Number(offset) || 0));
        this.data = d.slice(0, offset) + String(text) + d.slice(offset);
    }
    replaceData(offset, count, text) {
        this.deleteData(offset, count);
        this.insertData(offset, text);
    }
    substringData(offset, count) {
        let d = this.data;
        offset = Math.min(d.length, Math.max(0, Number(offset) || 0));
        count = Math.max(0, Number(count) || 0);
        return d.slice(offset, offset + count);
    }
}

class TextNode extends CharacterData {
    constructor(id) {
        super(id, 'text');
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

class Comment extends CharacterData {
    constructor(id, data) {
        super(id, 'comment');
        if (data !== undefined) this._data = String(data);
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
    getElementById(id) {
        let matches = this.querySelectorAll('#' + String(id));
        return matches.item(0);
    }
    get innerHTML() {
        return __aura_get_inner_html(this._id);
    }
    set innerHTML(val) {
        let oldChildren = Array.from(this.childNodes || []);
        __aura_set_inner_html(this._id, String(val));
        __aura_child_list_mutation(this, Array.from(this.childNodes || []), oldChildren, null, null);
    }
}

class ShadowRoot extends DocumentFragment {
    constructor(id, host, mode) {
        super(id);
        this.host = host;
        this.mode = mode === 'closed' ? 'closed' : 'open';
        this._isShadowRoot = true;
    }
    get parentNode() {
        return this.host;
    }
    get parentElement() {
        return this.host;
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

    get currentScript() {
        let scripts = document.scripts;
        if (!scripts || scripts.length === 0) return null;
        return scripts[scripts.length - 1] || null;
    },
    getAttribute: function(name) { return null; },
    setAttribute: function(name, value) {},
    removeAttribute: function(name) {},
    hasAttribute: function(name) { return false; },

    getElementById: function(id) {
        let res = __aura_get_element_by_id(id);
        return res ? __get_or_create_node(res.nid, res.tag, id, res.kind) : null;
    },
    createElement: function(tag) {
        let nativeId = __aura_create_element(tag);
        return __get_or_create_node(nativeId, tag, null, 'element');
    },
    createElementNS: function(ns, tag) {
        return document.createElement(tag);
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

    createTreeWalker: function(root, whatToShow, filter, expandEntityReferences) {
        return new TreeWalker(root, whatToShow, filter);
    },

    createNodeIterator: function(root, whatToShow, filter) {
        return new NodeIterator(root, whatToShow, filter);
    },

    createRange: function() {
        return new Range();
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
    get forms() {
        return new HTMLCollection(function() {
            return JSON.parse(__aura_get_elements_by_tag(0, 'form'));
        });
    },
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
    get styleSheets() {
        return new StyleSheetList(function() {
            let nids = JSON.parse(__aura_get_elements_by_tag(0, 'style'));
            return nids.map(nid => {
                let node = __get_or_create_node(nid, 'style', null, 'element');
                if (!node.sheet) {
                    node.sheet = new CSSStyleSheet(node.textContent || '');
                    node.sheet.ownerNode = node;
                }
                return node.sheet;
            });
        });
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
window.CSS = CSS;
window.CSSRule = CSSRule;
window.CSSStyleDeclaration = CSSStyleDeclaration;
window.CSSStyleRule = CSSStyleRule;
window.CSSStyleSheet = CSSStyleSheet;
window.StyleSheetList = StyleSheetList;
var console = { log: log, warn: warn, error: error, info: info, debug: debug };
var navigator = new EventTarget();
Object.assign(navigator, {
    userAgent: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
    appCodeName: 'Mozilla',
    appName: 'Netscape',
    appVersion: '5.0 (X11; Linux x86_64)',
    language: 'en-US',
    languages: ['en-US', 'en'],
    platform: 'Linux x86_64',
    cookieEnabled: true,
    onLine: true,
    hardwareConcurrency: 4,
    maxTouchPoints: 0,
    vendor: 'Google Inc.',
    vendorSub: '',
    productSub: '20030107',
    product: 'Gecko',
    doNotTrack: null,
    webdriver: false,
    plugins: [],
    mimeTypes: [],
    userAgentData: {
        brands: [{ brand: 'Chromium', version: '120' }, { brand: 'Aura', version: '1' }],
        mobile: false,
        platform: 'Linux',
        getHighEntropyValues: function(hints) {
            let values = { brands: this.brands, mobile: this.mobile, platform: this.platform };
            (hints || []).forEach(hint => {
                if (hint === 'architecture') values.architecture = 'x86';
                if (hint === 'bitness') values.bitness = '64';
                if (hint === 'model') values.model = '';
                if (hint === 'platformVersion') values.platformVersion = '';
            });
            return Promise.resolve(values);
        }
    },
    permissions: {
        query: function(desc) {
            return Promise.resolve({ name: desc && desc.name || '', state: 'prompt', onchange: null });
        }
    },
    clipboard: {
        readText: function() { return Promise.resolve(''); },
        writeText: function() { return Promise.resolve(); }
    },
    geolocation: {
        getCurrentPosition: function(success, error) {
            if (typeof error === 'function') error({ code: 1, message: 'Geolocation is not available' });
        },
        watchPosition: function(success, error) {
            if (typeof error === 'function') error({ code: 1, message: 'Geolocation is not available' });
            return 0;
        },
        clearWatch: function(id) {}
    },
    javaEnabled: function() { return false; },
    sendBeacon: function(url, data) { return true; },
    vibrate: function(pattern) { return false; },
});
window.navigator = navigator;
var location = document.location;

// -- window dimensions -------------------------------------------------------
window.self = window;
window.window = window;
window.top = window;
window.parent = window;
window.frames = window;
window.length = 0;
window.name = '';
window.status = '';
window.closed = false;
window.opener = null;
window.frameElement = null;
window.innerWidth = 800;
window.innerHeight = 600;
window.outerWidth = 800;
window.outerHeight = 600;
window.screenX = 0;
window.screenY = 0;
window.screenLeft = 0;
window.screenTop = 0;
let __aura_screen_orientation = new EventTarget();
Object.assign(__aura_screen_orientation, {
    type: 'landscape-primary',
    angle: 0,
    onchange: null,
    lock: function(type) { return Promise.reject(new Error('Screen orientation lock is not supported')); },
    unlock: function() {}
});
window.screen = new EventTarget();
Object.assign(window.screen, {
    width: 800,
    height: 600,
    availWidth: 800,
    availHeight: 600,
    availLeft: 0,
    availTop: 0,
    colorDepth: 24,
    pixelDepth: 24,
    isExtended: false,
    orientation: __aura_screen_orientation,
    onchange: null,
});
window.devicePixelRatio = 1;
window.scrollX = 0;
window.scrollY = 0;
window.pageXOffset = 0;
window.pageYOffset = 0;
window.visualViewport = new EventTarget();
Object.assign(window.visualViewport, {
    width: 800,
    height: 600,
    scale: 1,
    offsetLeft: 0,
    offsetTop: 0,
    pageLeft: 0,
    pageTop: 0,
    onresize: null,
    onscroll: null,
});
window.locationbar = { visible: true };
window.menubar = { visible: true };
window.personalbar = { visible: true };
window.scrollbars = { visible: true };
window.statusbar = { visible: true };
window.toolbar = { visible: true };
window.__aura_environmentNotes = {
    unsupported: [
        'serviceWorker',
        'indexedDB',
        'webgl',
        'webgpu',
        'notifications',
        'real clipboard access',
        'real geolocation'
    ],
    viewport: 'Fixed 800x600 CSS pixel viewport in the current headless runtime.'
};
window.__aura_inline_script_allowed = true;

// -- history -----------------------------------------------------------------
window.history = {
    length: 1,
    state: null,
    _entries: [location.href],
    _states: [null],
    _index: 0,
    _ensureCurrentEntry: function() {
        if (this._entries.length === 1 && this._index === 0 && this._entries[0] !== location.href) {
            this._entries[0] = location.href;
        }
    },
    _dispatchNavigationEvents: function(oldURL, oldHash, newURL, newHash) {
        window.dispatchEvent(new PopStateEvent('popstate', { state: this.state }));
        if (oldHash !== newHash) {
            window.dispatchEvent(new HashChangeEvent('hashchange', { oldURL, newURL }));
        }
    },
    _traverseTo: function(index) {
        this._ensureCurrentEntry();
        if (index < 0 || index >= this._entries.length || index === this._index) return;
        let oldURL = location.href;
        let oldHash = location.hash;
        this._index = index;
        this.state = this._states[this._index];
        __aura_apply_location_href(this._entries[this._index]);
        this._dispatchNavigationEvents(oldURL, oldHash, location.href, location.hash);
    },
    pushState: function(state, title, url) {
        this._ensureCurrentEntry();
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
        this._states = this._states.slice(0, this._index + 1);
        this._entries.push(nextHref);
        this._states.push(state);
        this._index = this._entries.length - 1;
        this.length = this._entries.length;
    },
    replaceState: function(state, title, url) {
        this._ensureCurrentEntry();
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
        this._states[this._index] = state;
    },
    back: function() { this.go(-1); },
    forward: function() { this.go(1); },
    go: function(delta) {
        let step = delta === undefined ? 0 : Number(delta);
        if (!Number.isFinite(step)) step = 0;
        if (step === 0) return;
        this._traverseTo(this._index + Math.trunc(step));
    },
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

// -- document.cookie ----------------------------------------------------------
var __aura_cookie_jar = [];

function __aura_cookie_hostname() {
    return (location.hostname || '').toLowerCase();
}

function __aura_cookie_pathname() {
    return location.pathname || '/';
}

function __aura_cookie_default_path() {
    let path = __aura_cookie_pathname();
    if (!path || path[0] !== '/') return '/';
    let lastSlash = path.lastIndexOf('/');
    if (lastSlash <= 0) return '/';
    return path.slice(0, lastSlash);
}

function __aura_cookie_domain_match(host, domain, hostOnly) {
    host = String(host || '').toLowerCase();
    domain = String(domain || '').toLowerCase();
    if (hostOnly) return host === domain;
    return host === domain || host.endsWith('.' + domain);
}

function __aura_cookie_path_match(requestPath, cookiePath) {
    requestPath = requestPath || '/';
    cookiePath = cookiePath || '/';
    if (requestPath === cookiePath) return true;
    if (!requestPath.startsWith(cookiePath)) return false;
    return cookiePath.endsWith('/') || requestPath[cookiePath.length] === '/';
}

function __aura_cookie_is_expired(cookie, now) {
    return cookie.expires !== null && cookie.expires <= now;
}

function __aura_cookie_prune() {
    let now = Date.now();
    __aura_cookie_jar = __aura_cookie_jar.filter(cookie => !__aura_cookie_is_expired(cookie, now));
}

function __aura_cookie_visible(cookie) {
    return !cookie.httpOnly
        && (!cookie.secure || location.protocol === 'https:')
        && __aura_cookie_domain_match(__aura_cookie_hostname(), cookie.domain, cookie.hostOnly)
        && __aura_cookie_path_match(__aura_cookie_pathname(), cookie.path);
}

function __aura_cookie_parse_expiry(value) {
    let time = Date.parse(value);
    return Number.isNaN(time) ? null : time;
}

Object.defineProperty(document, 'cookie', {
    get: function() {
        __aura_cookie_prune();
        return __aura_cookie_jar
            .filter(__aura_cookie_visible)
            .map(cookie => cookie.name + '=' + cookie.value)
            .join('; ');
    },
    set: function(val) {
        let input = String(val == null ? '' : val);
        let parts = input.split(';');
        let pair = parts.shift();
        if (!pair) return;

        let eq = pair.indexOf('=');
        if (eq <= 0) return;

        let name = pair.slice(0, eq).trim();
        if (!name || /[\x00-\x20\x7f()<>@,;:\\"\/\[\]?={}]/.test(name)) return;

        let host = __aura_cookie_hostname();
        if (!host) return;

        let value = pair.slice(eq + 1).trim();
        let cookie = {
            name: name,
            value: value,
            domain: host,
            hostOnly: true,
            path: __aura_cookie_default_path(),
            expires: null,
            secure: false,
            sameSite: '',
            httpOnly: false,
        };

        for (let attr of parts) {
            let trimmed = attr.trim();
            if (!trimmed) continue;
            let attrEq = trimmed.indexOf('=');
            let attrName = (attrEq >= 0 ? trimmed.slice(0, attrEq) : trimmed).trim().toLowerCase();
            let attrValue = attrEq >= 0 ? trimmed.slice(attrEq + 1).trim() : '';

            if (attrName === 'domain') {
                let domain = attrValue.toLowerCase().replace(/^\./, '');
                if (domain && __aura_cookie_domain_match(host, domain, false)) {
                    cookie.domain = domain;
                    cookie.hostOnly = false;
                } else {
                    return;
                }
            } else if (attrName === 'path') {
                cookie.path = attrValue && attrValue[0] === '/' ? attrValue : '/';
            } else if (attrName === 'expires') {
                let expires = __aura_cookie_parse_expiry(attrValue);
                if (expires !== null) cookie.expires = expires;
            } else if (attrName === 'max-age') {
                let seconds = Number(attrValue);
                if (Number.isFinite(seconds)) cookie.expires = Date.now() + Math.trunc(seconds) * 1000;
            } else if (attrName === 'secure') {
                cookie.secure = true;
            } else if (attrName === 'samesite') {
                cookie.sameSite = attrValue;
            }
        }

        __aura_cookie_jar = __aura_cookie_jar.filter(existing =>
            !(existing.name === cookie.name
                && existing.domain === cookie.domain
                && existing.path === cookie.path
                && existing.hostOnly === cookie.hostOnly)
        );

        if (!__aura_cookie_is_expired(cookie, Date.now())) {
            __aura_cookie_jar.push(cookie);
        }
    },
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
    let computed = new CSSStyleDeclaration(null, false);
    if (el && el.style) {
        for (let i = 0; i < el.style.length; i++) {
            let name = el.style.item(i);
            computed._props.push({
                name,
                value: el.style.getPropertyValue(name),
                priority: el.style.getPropertyPriority(name)
            });
            computed[__aura_css_kebab_to_camel(name)] = el.style.getPropertyValue(name);
        }
    }
    computed._readonly = true;
    return computed;
};

// -- XMLHttpRequest ----------------------------------------------------------
class XMLHttpRequest extends EventTarget {
    constructor() {
        super();
        this.UNSENT = 0;
        this.OPENED = 1;
        this.HEADERS_RECEIVED = 2;
        this.LOADING = 3;
        this.DONE = 4;
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
        this.onloadend = null;
        this.onloadstart = null;
        this._method = '';
        this._url = '';
        this._headers = new Headers();
        this._responseHeaders = new Headers();
        this._async = true;
        this._sent = false;
        this._aborted = false;
    }
    _fire(type) {
        let event = new Event(type);
        this.dispatchEvent(event);
    }
    _changeReadyState(state) {
        this.readyState = state;
        this._fire('readystatechange');
    }
    open(method, url, async = true, user, password) {
        this._method = String(method || 'GET').toUpperCase();
        try {
            this._url = new URL(String(url), location.href || document.baseURI || 'about:blank').href;
        } catch (e) {
            this._url = String(url);
        }
        this._async = async !== false;
        this._headers = new Headers();
        this._responseHeaders = new Headers();
        this._sent = false;
        this._aborted = false;
        this.status = 0;
        this.statusText = '';
        this.responseText = '';
        this.response = null;
        this.readyState = 1;
        this._fire('readystatechange');
    }
    send(body) {
        if (this.readyState !== this.OPENED || this._sent) {
            throw new Error('XMLHttpRequest is not open');
        }
        if (!this._async) {
            throw new Error('Synchronous XMLHttpRequest is not supported');
        }

        this._sent = true;
        this._aborted = false;
        this._fire('loadstart');

        fetch(this._url, {
            method: this._method,
            headers: this._headers,
            body: body === undefined || body === null ? undefined : body,
            credentials: this.withCredentials ? 'include' : 'same-origin'
        }).then(response => {
            if (this._aborted) return;
            this.status = response.status;
            this.statusText = response.statusText;
            this.responseURL = response.url || this._url;
            this._responseHeaders = new Headers(response.headers);
            this._changeReadyState(this.HEADERS_RECEIVED);
            this._changeReadyState(this.LOADING);
            return response.text();
        }).then(text => {
            if (this._aborted || text === undefined) return;
            this.responseText = String(text);
            if (this.responseType === '' || this.responseType === 'text') {
                this.response = this.responseText;
            } else if (this.responseType === 'json') {
                try { this.response = this.responseText ? JSON.parse(this.responseText) : null; }
                catch (e) { this.response = null; }
            } else {
                this.response = this.responseText;
            }
            this._changeReadyState(this.DONE);
            this._fire('load');
            this._fire('loadend');
        }).catch(error => {
            if (this._aborted) return;
            this.status = 0;
            this.statusText = '';
            this.responseText = '';
            this.response = null;
            this._changeReadyState(this.DONE);
            this._fire('error');
            this._fire('loadend');
        });
    }
    setRequestHeader(name, value) {
        if (this.readyState !== this.OPENED || this._sent) {
            throw new Error('XMLHttpRequest is not open');
        }
        this._headers.append(name, value);
    }
    getResponseHeader(name) {
        if (this.readyState < this.HEADERS_RECEIVED) return null;
        return this._responseHeaders.get(name);
    }
    getAllResponseHeaders() {
        if (this.readyState < this.HEADERS_RECEIVED) return '';
        return Array.from(this._responseHeaders)
            .map(pair => pair[0] + ': ' + pair[1] + '\r\n')
            .join('');
    }
    abort() {
        this._aborted = true;
        this._sent = false;
        this.status = 0;
        this.statusText = '';
        this.responseText = '';
        this.response = null;
        if (this.readyState !== this.UNSENT && this.readyState !== this.DONE) {
            this._changeReadyState(this.DONE);
        }
        this._fire('abort');
        this._fire('loadend');
    }
    overrideMimeType() {}
}
window.XMLHttpRequest = XMLHttpRequest;
XMLHttpRequest.UNSENT = 0;
XMLHttpRequest.OPENED = 1;
XMLHttpRequest.HEADERS_RECEIVED = 2;
XMLHttpRequest.LOADING = 3;
XMLHttpRequest.DONE = 4;

// -- window.matchMedia -------------------------------------------------------
window.matchMedia = function(query) {
    let mql = new EventTarget();
    Object.assign(mql, {
        matches: false,
        media: String(query),
        onchange: null,
        addListener: function(callback) { this.addEventListener('change', callback); },
        removeListener: function(callback) { this.removeEventListener('change', callback); },
    });
    return mql;
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

function __aura_post_message_impl(target, message, targetOrigin) {
    if (typeof target.dispatchEvent !== 'function') return;
    setTimeout(function() {
        try {
            var ev = new Event('message');
            ev.data = message;
            ev.origin = typeof targetOrigin === 'string' ? targetOrigin : '*';
            ev.source = target;
            target.dispatchEvent(ev);
        } catch (e) {
            console.error('Error dispatching message event: ' + e);
        }
    }, 0);
}
window.postMessage = function(message, targetOrigin, transfer) {
    __aura_post_message_impl(globalThis, message, targetOrigin);
};

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
class Headers {
    constructor(init) {
        this._headers = [];
        if (init instanceof Headers) {
            init.forEach((value, name) => this.append(name, value));
        } else if (Array.isArray(init)) {
            init.forEach(pair => this.append(pair[0], pair[1]));
        } else if (init && typeof init === 'object') {
            Object.keys(init).forEach(name => this.append(name, init[name]));
        }
    }
    _normalizeName(name) {
        let normalized = String(name).toLowerCase();
        if (!normalized || /[\x00-\x20\x7f()<>@,;:\\"\/\[\]?={}]/.test(normalized)) {
            throw new TypeError('Invalid header name');
        }
        return normalized;
    }
    _normalizeValue(value) {
        return String(value).trim();
    }
    append(name, value) {
        name = this._normalizeName(name);
        value = this._normalizeValue(value);
        let existing = this._headers.find(header => header[0] === name);
        if (existing) existing[1] += ', ' + value;
        else this._headers.push([name, value]);
    }
    delete(name) {
        name = this._normalizeName(name);
        this._headers = this._headers.filter(header => header[0] !== name);
    }
    get(name) {
        name = this._normalizeName(name);
        let header = this._headers.find(header => header[0] === name);
        return header ? header[1] : null;
    }
    has(name) {
        name = this._normalizeName(name);
        return this._headers.some(header => header[0] === name);
    }
    set(name, value) {
        name = this._normalizeName(name);
        value = this._normalizeValue(value);
        this.delete(name);
        this._headers.push([name, value]);
    }
    forEach(callback, thisArg) {
        this._headers.forEach(header => callback.call(thisArg, header[1], header[0], this));
    }
    keys() { return this._headers.map(header => header[0])[Symbol.iterator](); }
    values() { return this._headers.map(header => header[1])[Symbol.iterator](); }
    entries() { return this._headers.map(header => [header[0], header[1]])[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
}
window.Headers = Headers;

function __aura_body_to_string(body) {
    if (body === undefined || body === null) return '';
    if (body instanceof URLSearchParams) return body.toString();
    if (body instanceof Blob) return body._text || '';
    if (body instanceof FormData) {
        let params = new URLSearchParams();
        body.forEach((value, name) => params.append(name, value));
        return params.toString();
    }
    return String(body);
}

function __aura_encode_headers(headers) {
    let out = {};
    headers.forEach((value, name) => { out[name] = value; });
    return JSON.stringify(out);
}

function __aura_consumable_body(target, body) {
    target._body = body === undefined || body === null ? '' : String(body);
    target.bodyUsed = false;
    target._consumeBody = function() {
        if (this.bodyUsed) return Promise.reject(new TypeError('Body has already been used'));
        this.bodyUsed = true;
        return Promise.resolve(this._body);
    };
    target.text = function() { return this._consumeBody(); };
    target.json = function() { return this._consumeBody().then(text => JSON.parse(text)); };
    target.arrayBuffer = function() {
        return this._consumeBody().then(text => {
            let buffer = new ArrayBuffer(text.length);
            let view = new Uint8Array(buffer);
            for (let i = 0; i < text.length; i++) view[i] = text.charCodeAt(i) & 0xff;
            return buffer;
        });
    };
    target.blob = function() { return this._consumeBody().then(text => new Blob([text])); };
}

class Request {
    constructor(input, init = {}) {
        if (input instanceof Request) {
            this.url = input.url;
            this.method = input.method;
            this.headers = new Headers(input.headers);
            this.credentials = input.credentials;
            this.mode = input.mode;
            this.cache = input.cache;
            this.redirect = input.redirect;
            this.referrer = input.referrer;
            __aura_consumable_body(this, init.body !== undefined ? __aura_body_to_string(init.body) : input._body);
        } else {
            this.url = new URL(String(input), location.href).href;
            this.method = 'GET';
            this.headers = new Headers();
            this.credentials = 'same-origin';
            this.mode = 'cors';
            this.cache = 'default';
            this.redirect = 'follow';
            this.referrer = 'about:client';
            __aura_consumable_body(this, '');
        }

        if (init.method !== undefined) this.method = String(init.method).toUpperCase();
        if (init.headers !== undefined) this.headers = new Headers(init.headers);
        if (init.credentials !== undefined) this.credentials = String(init.credentials);
        if (init.mode !== undefined) this.mode = String(init.mode);
        if (init.cache !== undefined) this.cache = String(init.cache);
        if (init.redirect !== undefined) this.redirect = String(init.redirect);
        if (init.referrer !== undefined) this.referrer = String(init.referrer);
        if (init.body !== undefined) __aura_consumable_body(this, __aura_body_to_string(init.body));

        if ((this.method === 'GET' || this.method === 'HEAD') && this._body) {
            throw new TypeError('Request with GET/HEAD method cannot have body');
        }
    }
    clone() {
        if (this.bodyUsed) throw new TypeError('Body has already been used');
        return new Request(this);
    }
}
window.Request = Request;

class Response {
    constructor(body = null, init = {}) {
        this.status = init.status === undefined ? 200 : Number(init.status);
        this.statusText = init.statusText === undefined ? '' : String(init.statusText);
        this.headers = new Headers(init.headers);
        this.url = init.url || '';
        this.type = init.type || 'basic';
        this.redirected = !!init.redirected;
        this.ok = this.status >= 200 && this.status <= 299;
        __aura_consumable_body(this, __aura_body_to_string(body));
    }
    clone() {
        if (this.bodyUsed) throw new TypeError('Body has already been used');
        return new Response(this._body, {
            status: this.status,
            statusText: this.statusText,
            headers: this.headers,
            url: this.url,
            type: this.type,
            redirected: this.redirected,
        });
    }
    static json(data, init = {}) {
        let headers = new Headers(init.headers);
        if (!headers.has('content-type')) headers.set('content-type', 'application/json');
        return new Response(JSON.stringify(data), Object.assign({}, init, { headers }));
    }
    static redirect(url, status = 302) {
        return new Response(null, { status, headers: { location: new URL(String(url), location.href).href } });
    }
    static error() {
        return new Response(null, { status: 0, statusText: '', type: 'error' });
    }
}
window.Response = Response;

function __aura_make_fetch_response(data) {
    return new Response(data.body || '', {
        status: data.status || 0,
        statusText: data.statusText || '',
        headers: data.headers || {},
        url: data.url || '',
        type: data.type || 'basic',
        redirected: !!data.redirected,
    });
}

window.fetch = function(input, init) {
    let request;
    try {
        request = new Request(input, init || {});
    } catch (e) {
        return Promise.reject(e);
    }
    return new Promise((resolve, reject) => {
        __aura_fetch(
            request.url,
            request.method,
            __aura_encode_headers(request.headers),
            request._body,
            resolve,
            reject
        );
    });
};

function __aura_script_type(script) {
    return (script.getAttribute('type') || '').trim().toLowerCase();
}

function __aura_script_fire(script, type) {
    __aura_queue_task(() => script.dispatchEvent(new Event(type)));
}

function __aura_execute_classic_script(script, code) {
    try {
        (0, eval)(String(code || ''));
        __aura_script_fire(script, 'load');
    } catch (e) {
        console.error('Script execution error: ' + e);
        __aura_script_fire(script, 'error');
    }
}

function __aura_execute_module_script(script, url, code) {
    try {
        let success = __aura_execute_module_script_in_host(url, code);
        if (success) {
            __aura_script_fire(script, 'load');
        } else {
            __aura_script_fire(script, 'error');
        }
    } catch (e) {
        console.error('Module script execution error: ' + e);
        __aura_script_fire(script, 'error');
    }
}

function __aura_maybe_run_script(node) {
    if (!node || node.nodeType !== Node.ELEMENT_NODE || String(node.tagName || '').toLowerCase() !== 'script') return;
    let script = node;
    if (script._alreadyStarted) return;
    script._alreadyStarted = true;

    let type = __aura_script_type(script);
    let isModule = type === 'module';
    if (type && type !== 'text/javascript' && type !== 'application/javascript' && type !== 'classic' && !isModule) {
        console.warn('Unsupported script type skipped: ' + type);
        __aura_script_fire(script, 'error');
        return;
    }

    let src = script.src;
    if (src) {
        if (typeof __aura_can_execute_script_url === 'function' && !__aura_can_execute_script_url(src)) {
            console.warn('CSP blocked dynamic script: ' + src);
            __aura_script_fire(script, 'error');
            return;
        }
        fetch(src)
            .then(response => response.ok ? response.text() : Promise.reject(new Error('HTTP ' + response.status)))
            .then(code => {
                if (isModule) {
                    __aura_execute_module_script(script, src, code);
                } else {
                    __aura_execute_classic_script(script, code);
                }
            })
            .catch(error => {
                console.error((isModule ? 'Module script' : 'Script') + ' load error: ' + error);
                __aura_script_fire(script, 'error');
            });
        return;
    }

    if (!isModule && !window.__aura_inline_script_allowed) {
        console.warn('CSP blocked inline dynamic script');
        __aura_script_fire(script, 'error');
        return;
    }
    if (isModule) {
        let inline_url = window.location.href;
        __aura_execute_module_script(script, inline_url, script.text || script.textContent || '');
    } else {
        __aura_execute_classic_script(script, script.text || script.textContent || '');
    }
}

// -- MutationObserver --------------------------------------------------------
class MutationObserver {
    constructor(callback) {
        if (typeof callback !== 'function') throw new TypeError('MutationObserver callback must be a function');
        this._callback = callback;
        this._registrations = [];
        this._records = [];
        __aura_mutation_observers.push(this);
    }
    observe(target, options) {
        options = options || {};
        let normalized = {
            childList: !!options.childList,
            attributes: !!options.attributes || !!options.attributeOldValue || !!options.attributeFilter,
            characterData: !!options.characterData || !!options.characterDataOldValue,
            subtree: !!options.subtree,
            attributeOldValue: !!options.attributeOldValue,
            characterDataOldValue: !!options.characterDataOldValue,
            attributeFilter: options.attributeFilter ? Array.from(options.attributeFilter).map(String) : null
        };
        this._registrations = this._registrations.filter(registration => registration.target !== target);
        this._registrations.push({ target, options: normalized });
        if (!__aura_mutation_observers.includes(this)) __aura_mutation_observers.push(this);
    }
    disconnect() {
        this._registrations = [];
        this._records = [];
        __aura_mutation_observers = __aura_mutation_observers.filter(observer => observer !== this);
    }
    takeRecords() {
        let records = this._records.slice();
        this._records.length = 0;
        return records;
    }
}

// -- IntersectionObserver stub -----------------------------------------------
class IntersectionObserver {
    constructor(callback, options) { this._callback = callback; }
    observe(target) {}
    unobserve(target) {}
    disconnect() {}
    takeRecords() { return []; }
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

// -- URL and URLSearchParams -------------------------------------------------
function __aura_encode_query_part(value) {
    return encodeURIComponent(String(value)).replace(/%20/g, '+');
}

function __aura_decode_query_part(value) {
    return decodeURIComponent(String(value).replace(/\+/g, ' '));
}

class URLSearchParams {
    constructor(init, updateCallback) {
        this._pairs = [];
        this._updateCallback = typeof updateCallback === 'function' ? updateCallback : null;
        this._replaceFrom(init);
    }
    _replaceFrom(init) {
        this._pairs = [];
        if (init instanceof URLSearchParams) {
            for (let pair of init) this._pairs.push([pair[0], pair[1]]);
        } else if (Array.isArray(init)) {
            for (let pair of init) {
                if (pair && pair.length >= 2) this._pairs.push([String(pair[0]), String(pair[1])]);
            }
        } else if (init && typeof init === 'object' && typeof init !== 'string') {
            for (let key of Object.keys(init)) this._pairs.push([String(key), String(init[key])]);
        } else if (init !== undefined && init !== null) {
            let query = String(init).replace(/^\?/, '');
            if (query.length > 0) {
                for (let part of query.split('&')) {
                    if (part === '') continue;
                    let eq = part.indexOf('=');
                    let key = eq >= 0 ? part.slice(0, eq) : part;
                    let value = eq >= 0 ? part.slice(eq + 1) : '';
                    this._pairs.push([__aura_decode_query_part(key), __aura_decode_query_part(value)]);
                }
            }
        }
    }
    _replaceFromString(query) {
        this._replaceFrom(query || '');
    }
    _changed() {
        if (this._updateCallback) this._updateCallback(this.toString());
    }
    append(name, value) {
        this._pairs.push([String(name), String(value)]);
        this._changed();
    }
    delete(name, value) {
        name = String(name);
        let hasValue = arguments.length > 1;
        let expected = String(value);
        this._pairs = this._pairs.filter(pair => pair[0] !== name || (hasValue && pair[1] !== expected));
        this._changed();
    }
    get(name) {
        name = String(name);
        let pair = this._pairs.find(pair => pair[0] === name);
        return pair ? pair[1] : null;
    }
    getAll(name) {
        name = String(name);
        return this._pairs.filter(pair => pair[0] === name).map(pair => pair[1]);
    }
    has(name, value) {
        name = String(name);
        if (arguments.length > 1) {
            let expected = String(value);
            return this._pairs.some(pair => pair[0] === name && pair[1] === expected);
        }
        return this._pairs.some(pair => pair[0] === name);
    }
    set(name, value) {
        name = String(name);
        value = String(value);
        let replaced = false;
        let next = [];
        for (let pair of this._pairs) {
            if (pair[0] === name) {
                if (!replaced) {
                    next.push([name, value]);
                    replaced = true;
                }
            } else {
                next.push(pair);
            }
        }
        if (!replaced) next.push([name, value]);
        this._pairs = next;
        this._changed();
    }
    sort() {
        this._pairs.sort((a, b) => a[0] < b[0] ? -1 : (a[0] > b[0] ? 1 : 0));
        this._changed();
    }
    toString() {
        return this._pairs.map(pair => __aura_encode_query_part(pair[0]) + '=' + __aura_encode_query_part(pair[1])).join('&');
    }
    forEach(fn, thisArg) {
        for (let pair of this._pairs.slice()) fn.call(thisArg, pair[1], pair[0], this);
    }
    keys() { return this._iterator(0); }
    values() { return this._iterator(1); }
    entries() { return this._iterator(null); }
    _iterator(index) {
        let pairs = this._pairs.slice();
        let i = 0;
        return {
            next: function() {
                if (i >= pairs.length) return { done: true };
                let pair = pairs[i++];
                return { value: index === null ? [pair[0], pair[1]] : pair[index], done: false };
            },
            [Symbol.iterator]() { return this; }
        };
    }
    [Symbol.iterator]() { return this.entries(); }
    get size() { return this._pairs.length; }
}
window.URLSearchParams = URLSearchParams;

class URL {
    constructor(url, base) {
        this._searchParams = null;
        this._setHref(String(url), base === undefined || base === null || base === '' ? null : String(base));
    }
    _setHref(href, base) {
        let parsed = __aura_parse_url(href, base);
        if (!parsed) throw new TypeError('Invalid URL');
        this._initializing = true;
        this._href = parsed.href || '';
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
        this._initializing = false;
        this._href = this._compose();
        if (this._searchParams) this._searchParams._replaceFromString(this.search);
    }
    _compose() {
        return this.protocol + '//' + this.host + (this.pathname || '/') + (this.search || '') + (this.hash || '');
    }
    _updateHref() {
        if (!this._initializing && this._protocol && this._host !== undefined) {
            this._href = this._compose();
            this.origin = this.protocol + '//' + this.host;
        }
    }
    _reparse() {
        this._setHref(this._compose(), null);
    }
    get href() { return this._href; }
    set href(value) { this._setHref(String(value), null); }
    set protocol(value) { this._protocol = String(value).replace(/:$/, '') + ':'; this._updateHref(); }
    get protocol() { return this._protocol; }
    set host(value) {
        this._host = String(value);
        let parts = this._host.split(':');
        this._hostname = parts[0] || '';
        this._port = parts[1] || '';
        this._updateHref();
    }
    get host() { return this._host; }
    set hostname(value) {
        this._hostname = String(value);
        this._host = this._hostname + (this._port ? ':' + this._port : '');
        this._updateHref();
    }
    get hostname() { return this._hostname; }
    set port(value) {
        this._port = String(value || '');
        this._host = this._hostname + (this._port ? ':' + this._port : '');
        this._updateHref();
    }
    get port() { return this._port; }
    set pathname(value) {
        let path = String(value || '/');
        this._pathname = path.startsWith('/') ? path : '/' + path;
        this._updateHref();
    }
    get pathname() { return this._pathname; }
    set search(value) {
        let search = String(value || '');
        this._search = search === '' ? '' : (search.startsWith('?') ? search : '?' + search);
        if (this._searchParams) this._searchParams._replaceFromString(this._search);
        this._updateHref();
    }
    get search() { return this._search; }
    set hash(value) {
        let hash = String(value || '');
        this._hash = hash === '' ? '' : (hash.startsWith('#') ? hash : '#' + hash);
        this._updateHref();
    }
    get hash() { return this._hash; }
    set origin(value) { this._origin = String(value || ''); }
    get origin() { return this._origin; }
    get searchParams() {
        if (!this._searchParams) {
            this._searchParams = new URLSearchParams(this.search, query => {
                this._search = query ? '?' + query : '';
                this._href = this._compose();
            });
        }
        return this._searchParams;
    }
    toString() { return this.href; }
    toJSON() { return this.href; }
    static createObjectURL(blob) { return 'blob:'; }
    static revokeObjectURL(url) {}
}
window.URL = URL;

class Location {
    constructor(href) {
        this._url = new URL(href || 'about:blank');
    }
    _setHref(href, base) {
        this._url = new URL(href, base || this.href);
        document.URL = this.href;
        document.documentURI = this.href;
        document.baseURI = this.href;
    }
    get href() { return this._url.href; }
    set href(value) { this._setHref(String(value), this.href); }
    get protocol() { return this._url.protocol; }
    set protocol(value) { this._url.protocol = value; this._url._reparse(); this._setHref(this._url.href); }
    get host() { return this._url.host; }
    set host(value) { this._url.host = value; this._url._reparse(); this._setHref(this._url.href); }
    get hostname() { return this._url.hostname; }
    set hostname(value) { this._url.hostname = value; this._url._reparse(); this._setHref(this._url.href); }
    get port() { return this._url.port; }
    set port(value) { this._url.port = value; this._url._reparse(); this._setHref(this._url.href); }
    get pathname() { return this._url.pathname; }
    set pathname(value) { this._url.pathname = value; this._url._reparse(); this._setHref(this._url.href); }
    get search() { return this._url.search; }
    set search(value) { this._url.search = value; this._url._reparse(); this._setHref(this._url.href); }
    get hash() { return this._url.hash; }
    set hash(value) { this._url.hash = value; this._url._reparse(); this._setHref(this._url.href); }
    get origin() { return this._url.origin; }
    assign(url) { this.href = new URL(String(url), this.href).href; }
    replace(url) { this.assign(url); }
    reload() {}
    toString() { return this.href; }
}

function __aura_create_location(href) {
    return new Location(href);
}

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
window.CharacterData = CharacterData;
window.Text = TextNode;
window.Comment = Comment;
window.DocumentType = DocumentType;
window.DocumentFragment = DocumentFragment;
window.ShadowRoot = ShadowRoot;
window.NodeList = NodeList;
window.HTMLCollection = HTMLCollection;
window.NodeFilter = NodeFilter;
window.TreeWalker = TreeWalker;
window.NodeIterator = NodeIterator;
window.Range = Range;
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
    get src() { return __aura_resolve_url_attribute(__aura_get_attribute(this._id, 'src') || ''); }
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
    get href() { return __aura_resolve_url_attribute(__aura_get_attribute(this._id, 'href') || ''); }
    set href(v) { __aura_set_attribute(this._id, 'href', String(v)); }

    _getURL(for_write) {
        var raw = __aura_get_attribute(this._id, 'href');
        if (raw === null && !for_write) {
            return null;
        }
        try {
            return new URL(raw || '', location.href || document.baseURI || undefined);
        } catch (e) {
            return null;
        }
    }
    _setURLProp(prop, value) {
        var url = this._getURL(true);
        if (!url) return;
        url[prop] = value;
        this.href = url.href;
    }

    get protocol() { var url = this._getURL(); return url ? url.protocol : ''; }
    set protocol(v) { this._setURLProp('protocol', v); }

    get host() { var url = this._getURL(); return url ? url.host : ''; }
    set host(v) { this._setURLProp('host', v); }

    get hostname() { var url = this._getURL(); return url ? url.hostname : ''; }
    set hostname(v) { this._setURLProp('hostname', v); }

    get port() { var url = this._getURL(); return url ? url.port : ''; }
    set port(v) { this._setURLProp('port', v); }

    get pathname() { var url = this._getURL(); return url ? url.pathname : ''; }
    set pathname(v) { this._setURLProp('pathname', v); }

    get search() { var url = this._getURL(); return url ? url.search : ''; }
    set search(v) { this._setURLProp('search', v); }

    get hash() { var url = this._getURL(); return url ? url.hash : ''; }
    set hash(v) { this._setURLProp('hash', v); }

    get origin() { var url = this._getURL(); return url ? url.origin : ''; }
}
window.HTMLAnchorElement = HTMLAnchorElement;

class HTMLScriptElement extends HTMLElement {
    get src() { return __aura_resolve_url_attribute(__aura_get_attribute(this._id, 'src') || ''); }
    set src(v) { __aura_set_attribute(this._id, 'src', String(v)); }
    get type() { return __aura_get_attribute(this._id, 'type') || ''; }
    set type(v) { __aura_set_attribute(this._id, 'type', String(v)); }
    get text() { return this.textContent; }
    set text(v) { this.textContent = String(v); }
    get async() { return __aura_has_attribute(this._id, 'async'); }
    set async(v) { if (v) __aura_set_attribute(this._id, 'async', ''); else __aura_remove_attribute(this._id, 'async'); }
    get defer() { return __aura_has_attribute(this._id, 'defer'); }
    set defer(v) { if (v) __aura_set_attribute(this._id, 'defer', ''); else __aura_remove_attribute(this._id, 'defer'); }
    get noModule() { return __aura_has_attribute(this._id, 'nomodule'); }
    set noModule(v) { if (v) __aura_set_attribute(this._id, 'nomodule', ''); else __aura_remove_attribute(this._id, 'nomodule'); }
    get crossOrigin() { return __aura_get_attribute(this._id, 'crossorigin'); }
    set crossOrigin(v) {
        if (v === null || v === undefined) __aura_remove_attribute(this._id, 'crossorigin');
        else __aura_set_attribute(this._id, 'crossorigin', String(v));
    }
}
window.HTMLScriptElement = HTMLScriptElement;

class HTMLStyleElement extends HTMLElement {}
window.HTMLStyleElement = HTMLStyleElement;

class HTMLIFrameElement extends HTMLElement {
    constructor(id, tag, string_id) {
        super(id, tag, string_id);
        this._contentDocument = null;
        this._contentWindow = null;
        this._container = null;
    }
    get contentDocument() {
        if (!this._contentDocument) {
            var self = this;
            this._contentDocument = {
                createElement: function(tag) { return document.createElement(tag); },
                createElementNS: function(ns, tag) { return document.createElement(tag); },
                createTextNode: function(text) { return document.createTextNode(text); },
                createComment: function(data) { return document.createComment(data); },
                createDocumentFragment: function() { return document.createDocumentFragment(); },
                getElementById: function(id) {
                    return self._container ? self._container.querySelector('#' + id) : null;
                },
                getElementsByClassName: function(cls) {
                    return self._container ? self._container.getElementsByClassName(cls) : new NodeList([]);
                },
                getElementsByTagName: function(tag) {
                    return self._container ? self._container.getElementsByTagName(String(tag).toLowerCase()) : new NodeList([]);
                },
                querySelector: function(sel) {
                    return self._container ? self._container.querySelector(sel) : null;
                },
                querySelectorAll: function(sel) {
                    return self._container ? self._container.querySelectorAll(sel) : new NodeList([]);
                },
                get head() {
                    if (!self._head) {
                        var html = self._container ? self._container.firstChild : null;
                        while (html && html.tagName !== 'HTML') html = html.nextSibling;
                        if (html) {
                            var child = html.firstChild;
                            while (child && child.tagName !== 'HEAD') child = child.nextSibling;
                            self._head = child || document.createElement('head');
                        } else {
                            self._head = document.createElement('head');
                        }
                    }
                    return self._head;
                },
                get body() {
                    if (!self._body) {
                        var html = self._container ? self._container.firstChild : null;
                        while (html && html.tagName !== 'HTML') html = html.nextSibling;
                        if (html) {
                            var child = html.lastChild;
                            while (child && child.tagName !== 'BODY') child = child.previousSibling;
                            self._body = child || document.createElement('body');
                        } else {
                            self._body = document.createElement('body');
                        }
                    }
                    return self._body;
                },
                get documentElement() {
                    return self._container ? self._container.firstChild : null;
                },
                get title() { return ''; },
                get readyState() { return 'complete'; },
                get URL() { return document.URL; },
                get baseURI() { return document.baseURI; },
                get documentURI() { return document.documentURI; },
                get nodeType() { return 9; },
                get nodeName() { return '#document'; },
                get cookie() { return ''; },
                set cookie(v) {},
                createTreeWalker: function(root, whatToShow, filter) {
                    return new TreeWalker(root, whatToShow, filter);
                },
                createNodeIterator: function(root, whatToShow, filter) {
                    return new NodeIterator(root, whatToShow, filter);
                },
                createRange: function() { return new Range(); },
                importNode: function(node, deep) { return node; },
                adoptNode: function(node) { return node; },
                execCommand: function() { return false; },
            };
        }
        return this._contentDocument;
    }
    get contentWindow() {
        if (!this._contentWindow) {
            var self = this;
            this._contentWindow = new EventTarget();
            var doc = self.contentDocument;
            Object.defineProperty(self._contentWindow, 'document', {
                get: function() { return doc; },
                enumerable: true,
                configurable: true
            });
            self._contentWindow.self = self._contentWindow;
            self._contentWindow.top = self._contentWindow;
            self._contentWindow.parent = window;
            self._contentWindow.frames = self._contentWindow;
            self._contentWindow.length = 0;
            self._contentWindow.name = '';
            self._contentWindow.location = document.location;
            self._contentWindow.closed = false;
            self._contentWindow.opener = null;
            self._contentWindow.frameElement = self;
            self._contentWindow.postMessage = function(message, targetOrigin, transfer) {
                __aura_post_message_impl(self._contentWindow, message, targetOrigin);
            };
        }
        return this._contentWindow;
    }
    appendChild(child) {
        if (!this._container) {
            this._container = document.createElement('div');
            this._container.style.display = 'none';
            var html = document.createElement('html');
            var head = document.createElement('head');
            var body = document.createElement('body');
            html.appendChild(head);
            html.appendChild(body);
            this._container.appendChild(html);
            this._head = head;
            this._body = body;
        }
        if (child && child._id !== undefined && child._id !== null) {
            return this._body.appendChild(child);
        }
        return child;
    }
}
window.HTMLIFrameElement = HTMLIFrameElement;

class HTMLFormElement extends HTMLElement {
    _controls() {
        var self = this;
        var nids = [];
        if (self._id) {
            try { nids = nids.concat(JSON.parse(__aura_get_elements_by_tag(self._id, 'input'))); } catch(e) {}
            try { nids = nids.concat(JSON.parse(__aura_get_elements_by_tag(self._id, 'select'))); } catch(e) {}
            try { nids = nids.concat(JSON.parse(__aura_get_elements_by_tag(self._id, 'textarea'))); } catch(e) {}
            try { nids = nids.concat(JSON.parse(__aura_get_elements_by_tag(self._id, 'button'))); } catch(e) {}
        }
        return nids;
    }

    get elements() {
        var self = this;
        return new HTMLCollection(function() {
            return self._controls();
        });
    }

    submit() {
        var event = new Event('submit', { bubbles: true, cancelable: true });
        var dispatched = this.dispatchEvent(event);
        if (dispatched && typeof __aura_submit_form === 'function') {
            __aura_submit_form(this._id);
        }
    }

    reset() {
        var controls = this._controls();
        for (var i = 0; i < controls.length; i++) {
            var node = __get_or_create_node(controls[i].nid, controls[i].tag, controls[i].id, controls[i].kind);
            if (!node) continue;
            if (node.tagName === 'INPUT' || node.tagName === 'TEXTAREA') {
                node.value = node.defaultValue;
                node.checked = node.defaultChecked;
            } else if (node.tagName === 'SELECT') {
                var options = Array.from(node.options);
                for (var j = 0; j < options.length; j++) {
                    options[j].selected = options[j].defaultSelected;
                }
            }
        }
    }

    requestSubmit(submitter) {
        var event = new Event('submit', { bubbles: true, cancelable: true });
        if (submitter) {
            event.submitter = submitter;
        }
        var dispatched = this.dispatchEvent(event);
        if (dispatched) {
            this.submit();
        }
    }
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


// -- atob / btoa stubs -------------------------------------------------------
window.atob = function(s) { return ''; };
window.btoa = function(s) { return ''; };

// -- window.scroll / scrollTo / scrollBy ------------------------------------
function __aura_set_scroll(x, y) {
    window.scrollX = Number(x) || 0;
    window.scrollY = Number(y) || 0;
    window.pageXOffset = window.scrollX;
    window.pageYOffset = window.scrollY;
    window.visualViewport.pageLeft = window.scrollX;
    window.visualViewport.pageTop = window.scrollY;
    window.dispatchEvent(new Event('scroll'));
    window.visualViewport.dispatchEvent(new Event('scroll'));
}
window.scroll = function(x, y) {
    if (typeof x === 'object' && x !== null) __aura_set_scroll(x.left || 0, x.top || 0);
    else __aura_set_scroll(x, y);
};
window.scrollTo = window.scroll;
window.scrollBy = function(x, y) {
    if (typeof x === 'object' && x !== null) __aura_set_scroll(window.scrollX + (Number(x.left) || 0), window.scrollY + (Number(x.top) || 0));
    else __aura_set_scroll(window.scrollX + (Number(x) || 0), window.scrollY + (Number(y) || 0));
};
window.alert = function(message) { console.log(String(message)); };
window.confirm = function(message) { console.log(String(message)); return false; };
window.prompt = function(message, defaultValue) { console.log(String(message)); return defaultValue === undefined ? null : String(defaultValue); };
window.print = function() {};
window.stop = function() {};
window.moveTo = function() {};
window.moveBy = function() {};
window.resizeTo = function(width, height) {
    window.outerWidth = Number(width) || window.outerWidth;
    window.outerHeight = Number(height) || window.outerHeight;
};
window.resizeBy = function(width, height) {
    window.outerWidth += Number(width) || 0;
    window.outerHeight += Number(height) || 0;
};

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
