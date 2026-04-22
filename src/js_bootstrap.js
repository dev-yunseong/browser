// Aura Browser JS bootstrap environment
// (Loaded at compile-time via include_bytes! in js.rs)

// -- Tracking state ----------------------------------------------------------
var __aura_style_log = [];
var __aura_inner_html_log = [];

function __aura_set_style(id, prop, value) {
    __aura_style_log.push(id + '||||' + prop + '||||' + value);
}

// -- Node Registry (Ensures stable objects for events) -----------------------
var __node_registry = new Map();

function __get_or_create_node(id, tag, string_id) {
    if (!id) return null;
    if (__node_registry.has(id)) return __node_registry.get(id);
    let node = tag ? new Element(id, tag, string_id) : new Node(id);
    __node_registry.set(id, node);
    return node;
}

// -- Event System ------------------------------------------------------------
class Event {
    constructor(type, options = {}) {
        this.type = type;
        this.bubbles = options.bubbles || false;
        this.cancelable = options.cancelable || false;
        this.target = null;
        this.currentTarget = null;
        this.defaultPrevented = false;
    }
    preventDefault() { this.defaultPrevented = true; }
    stopPropagation() { this._stopped = true; }
    stopImmediatePropagation() { this._stopped = true; this._immediateStopped = true; }
}

class MouseEvent extends Event {
    constructor(type, options = {}) {
        super(type, options);
        this.clientX = options.clientX || 0;
        this.clientY = options.clientY || 0;
    }
}

class EventTarget {
    constructor() {
        this._listeners = new Map();
    }
    addEventListener(type, callback) {
        if (!this._listeners.has(type)) this._listeners.set(type, []);
        this._listeners.get(type).push(callback);
    }
    removeEventListener(type, callback) {
        if (!this._listeners.has(type)) return;
        this._listeners.set(type, this._listeners.get(type).filter(l => l !== callback));
    }
    dispatchEvent(event) {
        event.target = this;
        let current = this;
        // Simple bubbling (if supported by event)
        while (current) {
            if (event._stopped) break;
            event.currentTarget = current;

            // 1. Check addEventListener listeners
            let list = current._listeners ? current._listeners.get(event.type) : null;
            if (list) {
                for (let listener of list) {
                    if (event._immediateStopped) break;
                    try { listener.call(current, event); } catch(e) { console.log("Event Error: " + e); }
                }
            }

            // 2. Check onEVENT property
            let onHandler = current['on' + event.type];
            if (typeof onHandler === 'function') {
                try { onHandler.call(current, event); } catch(e) { console.log("Event Error: " + e); }
            }

            if (!event.bubbles) break;
            current = current.parentNode;
        }
        return !event.defaultPrevented;
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
        this._nids = nids;
        this._tag = tag;
        for (let i = 0; i < nids.length; i++) {
            this[i] = __get_or_create_node(nids[i], null, null);
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

// -- Node & Element Classes ---------------------------------------------------
class Node extends EventTarget {
    constructor(id) {
        super();
        this._id = id;
    }
    get parentNode() {
        let pid = __aura_get_parent_id(this._id);
        if (!pid) return null;
        let info = __aura_get_node_info(pid);
        return info ? __get_or_create_node(pid, info.tag, info.id) : __get_or_create_node(pid);
    }
    get childNodes() {
        let arr = JSON.parse(__aura_get_children(this._id));
        return new NodeList(arr.map(c => c.nid));
    }
    get firstChild() {
        let arr = JSON.parse(__aura_get_children(this._id));
        if (arr.length === 0) return null;
        let c = arr[0];
        return __get_or_create_node(c.nid, c.tag, c.id);
    }
    get lastChild() {
        let arr = JSON.parse(__aura_get_children(this._id));
        if (arr.length === 0) return null;
        let c = arr[arr.length - 1];
        return __get_or_create_node(c.nid, c.tag, c.id);
    }
    get textContent() {
        return __aura_get_text_content(this._id);
    }
    set textContent(val) {
        __aura_set_text_content(this._id, String(val));
    }
    appendChild(child) {
        if (child && child._id) {
            __aura_append_child(this._id, child._id);
        }
        return child;
    }
    removeChild(child) {
        if (child && child._id) {
            __aura_remove_child(this._id, child._id);
        }
        return child;
    }
    insertBefore(newChild, refChild) {
        if (newChild && newChild._id) {
            __aura_insert_before(this._id, newChild._id, refChild ? refChild._id : null);
        }
        return newChild;
    }
    replaceChild(newChild, oldChild) {
        if (newChild && newChild._id && oldChild && oldChild._id) {
            __aura_insert_before(this._id, newChild._id, oldChild._id);
            __aura_remove_child(this._id, oldChild._id);
        }
        return oldChild;
    }
    cloneNode(deep) {
        // Shallow clone: create same-tag element, copy attributes
        let info = __aura_get_node_info(this._id);
        if (!info) return null;
        let clone = document.createElement(info.tag);
        // Copy all attributes via innerHTML trick is too complex; skip for now
        return clone;
    }
    contains(other) {
        if (!other || !other._id) return false;
        if (other._id === this._id) return true;
        let children = JSON.parse(__aura_get_children(this._id));
        for (let c of children) {
            let child_node = __get_or_create_node(c.nid, c.tag, c.id);
            if (child_node.contains(other)) return true;
        }
        return false;
    }
}

class Element extends Node {
    constructor(id, tag, string_id) {
        super(id);
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
        return info ? __get_or_create_node(nid, info.tag, info.id) : __get_or_create_node(nid);
    }
    querySelectorAll(selector) {
        let nids_json = __aura_query_selector_all(this._id, selector);
        let nids = JSON.parse(nids_json);
        return new NodeList(nids.map(nid => {
            let info = __aura_get_node_info(nid);
            return __get_or_create_node(nid, info ? info.tag : null, info ? info.id : null);
        }).map(el => el._id));
    }
    getElementsByClassName(cls) {
        let nids_json = __aura_get_elements_by_class(this._id, cls);
        let nids = JSON.parse(nids_json);
        return new NodeList(nids);
    }
    getElementsByTagName(tag) {
        let nids_json = __aura_get_elements_by_tag(this._id, tag.toLowerCase());
        let nids = JSON.parse(nids_json);
        return new NodeList(nids);
    }
    get children() {
        let arr = JSON.parse(__aura_get_children(this._id));
        return new NodeList(arr.map(c => {
            __get_or_create_node(c.nid, c.tag, c.id);
            return c.nid;
        }));
    }
    get childElementCount() {
        let arr = JSON.parse(__aura_get_children(this._id));
        return arr.length;
    }
    get firstElementChild() {
        let arr = JSON.parse(__aura_get_children(this._id));
        if (arr.length === 0) return null;
        let c = arr[0];
        return __get_or_create_node(c.nid, c.tag, c.id);
    }
    get lastElementChild() {
        let arr = JSON.parse(__aura_get_children(this._id));
        if (arr.length === 0) return null;
        let c = arr[arr.length - 1];
        return __get_or_create_node(c.nid, c.tag, c.id);
    }
    // getBoundingClientRect stub — returns zeros (layout info not available in JS)
    getBoundingClientRect() {
        return { x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0 };
    }
    // scrollIntoView stub
    scrollIntoView() {}
}

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
    addEventListener: function(type, callback) {
        if (!this._listeners.has(type)) this._listeners.set(type, []);
        this._listeners.get(type).push(callback);
        // DOMContentLoaded fires immediately (page already loaded)
        if (type === 'DOMContentLoaded') {
            try { callback(new Event('DOMContentLoaded')); } catch(e) {}
        }
    },
    removeEventListener: function(type, callback) {
        if (!this._listeners.has(type)) return;
        this._listeners.set(type, this._listeners.get(type).filter(l => l !== callback));
    },
    dispatchEvent: function(event) {
        event.target = this;
        let list = this._listeners.get(event.type);
        if (list) {
            for (let l of list) {
                try { l.call(this, event); } catch(e) { console.log("Event Error: " + e); }
            }
        }
        return !event.defaultPrevented;
    },

    getElementById: function(id) {
        let res = __aura_get_element_by_id(id);
        return res ? __get_or_create_node(res.nid, res.tag, id) : null;
    },
    createElement: function(tag) {
        let nativeId = __aura_create_element(tag);
        return __get_or_create_node(nativeId, tag);
    },
    createTextNode: function(text) {
        // Create a text node — for now return a minimal node-like object
        return { _id: 0, _text: text, nodeType: 3, textContent: text };
    },
    createDocumentFragment: function() {
        return document.createElement('div');
    },

    querySelector: function(selector) {
        let nid = __aura_query_selector(0, selector);
        if (!nid) return null;
        let info = __aura_get_node_info(nid);
        return info ? __get_or_create_node(nid, info.tag, info.id) : __get_or_create_node(nid);
    },
    querySelectorAll: function(selector) {
        let nids_json = __aura_query_selector_all(0, selector);
        let nids = JSON.parse(nids_json);
        let nodes = nids.map(nid => {
            let info = __aura_get_node_info(nid);
            return __get_or_create_node(nid, info ? info.tag : null, info ? info.id : null);
        });
        return new NodeList(nodes.map(n => n._id));
    },
    getElementsByClassName: function(cls) {
        let nids_json = __aura_get_elements_by_class(0, cls);
        let nids = JSON.parse(nids_json);
        return new NodeList(nids);
    },
    getElementsByTagName: function(tag) {
        let nids_json = __aura_get_elements_by_tag(0, tag.toLowerCase());
        let nids = JSON.parse(nids_json);
        return new NodeList(nids);
    },

    get body() {
        let nativeId = __aura_get_body();
        return nativeId ? __get_or_create_node(nativeId, 'body') : null;
    },
    get head() {
        let nids_json = __aura_get_elements_by_tag(0, 'head');
        let nids = JSON.parse(nids_json);
        if (nids.length === 0) return null;
        return __get_or_create_node(nids[0], 'head');
    },
    get documentElement() {
        let nids_json = __aura_get_elements_by_tag(0, 'html');
        let nids = JSON.parse(nids_json);
        if (nids.length === 0) return null;
        return __get_or_create_node(nids[0], 'html');
    },
    activeElement: null,
    location: { href: '', hostname: '', pathname: '/', search: '', hash: '' },
    title: '',
    readyState: 'complete',

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
var console = { log: log, warn: log, error: log, info: log, debug: log };
var navigator = { userAgent: 'Browser/2.0', language: 'en-US' };
var location = document.location;

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
