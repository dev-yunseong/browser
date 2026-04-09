// Aura Browser JS bootstrap environment
// (Loaded at compile-time via include_bytes! in js.rs)

// -- Tracking state ----------------------------------------------------------
var __aura_style_log = [];
var __aura_inner_html_log = [];

function __aura_set_style(id, prop, value) {
    __aura_style_log.push(id + '||||' + prop + '||||' + value);
}
function __aura_set_inner_html(id, html) {
    __aura_inner_html_log.push(id + '||||' + html);
}

// -- Basic globals ------------------------------------------------------------
var window = globalThis;
var console = { log: log, warn: log, error: log, info: log, debug: log };
var navigator = { userAgent: 'Browser/2.0', language: 'en-US', languages: ['en-US'] };

// -- Node & Element Classes ---------------------------------------------------
class Node {
    constructor(id) {
        this._id = id;
    }
    appendChild(child) {
        if (child instanceof Node) {
            __aura_append_child(this._id, child._id);
        }
        return child;
    }
}

class Element extends Node {
    constructor(id) {
        super(id);
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
}

// -- document -----------------------------------------------------------------
var document = {
    getElementById: function(id) {
        let nativeId = __aura_get_element_by_id(id);
        return nativeId ? new Element(nativeId) : null;
    },
    createElement: function(tag) {
        let nativeId = __aura_create_element(tag);
        return new Element(nativeId);
    },
    createTextNode: function(text) {
        let nativeId = __aura_create_text_node(text);
        return new Node(nativeId);
    },
    get body() {
        let nativeId = __aura_get_body();
        return nativeId ? new Element(nativeId) : null;
    },
    location: { href: '', hostname: '', pathname: '/', search: '', hash: '' },
    title: '',
    readyState: 'complete'
};

var location = document.location;

// -- Timers ------------------------------------------------------------------
var __timer_id = 0;
window.setTimeout = function(fn, delay) {
    __timer_id++;
    if (typeof fn === 'function') {
        __aura_queue_task(fn);
    } else if (typeof fn === 'string') {
        __aura_queue_task(() => eval(fn));
    }
    return __timer_id;
};

// -- fetch() -----------------------------------------------------------------
class Response {
    constructor(data) {
        this.status = data.status;
        this.ok = data.ok;
        this._body = data._body;
    }
    text() { return Promise.resolve(this._body); }
    json() { return Promise.resolve(JSON.parse(this._body)); }
}

window.fetch = function(url) {
    return new Promise((resolve, reject) => {
        __aura_fetch(url, resolve, reject);
    });
};

// -- Storage stubs -----------------------------------------------------------
window.localStorage = {
    _data: {},
    getItem: (k) => this._data[k] || null,
    setItem: (k, v) => { this._data[k] = String(v); },
    removeItem: (k) => { delete this._data[k]; },
    clear: () => { this._data = {}; }
};
window.sessionStorage = window.localStorage;
