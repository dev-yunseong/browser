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
            event.currentTarget = current;
            let list = current._listeners.get(event.type);
            if (list) {
                for (let listener of list) {
                    try { listener.call(current, event); } catch(e) { console.log("Event Error: " + e); }
                }
            }
            if (!event.bubbles) break;
            current = current.parentNode;
        }
        return !event.defaultPrevented;
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
        return pid ? __get_or_create_node(pid) : null;
    }
    appendChild(child) {
        if (child instanceof Node) {
            __aura_append_child(this._id, child._id);
        }
        return child;
    }
}

class Element extends Node {
    constructor(id, tag, string_id) {
        super(id);
        this.tagName = (tag || '').toUpperCase();
        this.id = string_id || '';
        this.style = new Proxy({ _id: id }, {
            set: (target, prop, value) => {
                let kebab = prop.replace(/([A-Z])/g, "-$1").toLowerCase();
                __aura_set_style(target._id, kebab, value);
                target[prop] = value;
                return true;
            }
        });
    }
    setAttribute(name, value) {
        __aura_set_attribute(this._id, name, String(value));
    }
    focus() {
        document.activeElement = this;
        __aura_set_focus(this.id);
    }
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
var document = {
    getElementById: function(id) {
        let nativeId = __aura_get_element_by_id(id);
        return nativeId ? __get_or_create_node(nativeId, null, id) : null;
    },
    createElement: function(tag) {
        let nativeId = __aura_create_element(tag);
        return __get_or_create_node(nativeId, tag);
    },
    get body() {
        let nativeId = __aura_get_body();
        return nativeId ? __get_or_create_node(nativeId, 'body') : null;
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

// -- fetch() -----------------------------------------------------------------
window.fetch = function(url) {
    return new Promise((resolve, reject) => {
        __aura_fetch(url, resolve, reject);
    });
};
