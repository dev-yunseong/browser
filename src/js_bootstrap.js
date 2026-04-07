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
var navigator = { userAgent: 'AuraBrowser/2.0', language: 'en-US', languages: ['en-US'] };

// -- document -----------------------------------------------------------------
var document = {
    getElementById: function(id) {
        return this._makeElement(id);
    },

    querySelector: function(sel) {
        if (sel && sel[0] === '#') {
            return this._makeElement(sel.slice(1));
        }
        return this._makeElement('__qs_' + sel);
    },

    querySelectorAll: function(sel) { return []; },
    getElementsByTagName: function(tag) { return []; },
    getElementsByClassName: function(cls) { return []; },
    createElement: function(tag) { return this._makeElement('__new_' + tag); },

    _makeElement: function(id) {
        var el = {
            _id: id,
            tagName: 'div',
            children: [],
            childNodes: [],
            classList: {
                _classes: [],
                add: function(c) { this._classes.push(c); },
                remove: function(c) {
                    var i = this._classes.indexOf(c);
                    if (i >= 0) this._classes.splice(i, 1);
                },
                toggle: function(c) {
                    var i = this._classes.indexOf(c);
                    if (i >= 0) this._classes.splice(i, 1); else this._classes.push(c);
                },
                contains: function(c) { return this._classes.indexOf(c) >= 0; }
            },
            getAttribute: function(name) { return null; },
            setAttribute: function(name, value) {},
            removeAttribute: function(name) {},
            addEventListener: function(type, handler, options) {},
            removeEventListener: function(type, handler) {},
            appendChild: function(child) { return child; },
            removeChild: function(child) { return child; },
            insertBefore: function(newNode, ref) { return newNode; },
            querySelector: function(sel) { return null; },
            querySelectorAll: function(sel) { return []; },
            focus: function() {},
            blur: function() {},
            click: function() {},
            getBoundingClientRect: function() {
                return { top: 0, left: 0, bottom: 0, right: 0, width: 0, height: 0 };
            },
            textContent: '',
            innerText: '',
            get innerHTML() { return ''; },
            set innerHTML(v) { __aura_set_inner_html(this._id, v); },
            style: (function(id) {
                var styleObj = { _id: id };
                var handler = {
                    set: function(target, prop, value) {
                        if (prop !== '_id') {
                            var kebab = prop.replace(/([A-Z])/g, function(m) { return '-' + m.toLowerCase(); });
                            __aura_set_style(target._id, kebab, value);
                        }
                        target[prop] = value;
                        return true;
                    },
                    get: function(target, prop) {
                        return target[prop] !== undefined ? target[prop] : '';
                    }
                };
                if (typeof Proxy !== 'undefined') {
                    return new Proxy(styleObj, handler);
                }
                return styleObj;
            })(id)
        };
        return el;
    },

    body: {
        style: {},
        appendChild: function(child) { return child; },
        innerHTML: '',
        children: [],
        classList: { add: function(){}, remove: function(){}, toggle: function(){} }
    },
    head: { appendChild: function(child) { return child; } },
    documentElement: { style: {}, scrollTop: 0, scrollLeft: 0 },
    location: { href: '', hostname: '', pathname: '/', search: '', hash: '' },
    title: '',
    readyState: 'complete',
    cookie: '',
    addEventListener: function(type, handler) {},
    removeEventListener: function(type, handler) {},
    createTextNode: function(text) { return { nodeType: 3, textContent: text }; },
    createComment: function(text) { return {}; },
    createDocumentFragment: function() {
        return { appendChild: function(c) { return c; }, children: [] };
    }
};

var location = document.location;

// -- Timers (fire immediately or no-op) --------------------------------------
var __timer_id = 0;
window.setTimeout = function(fn, delay) {
    __timer_id++;
    if (typeof fn === 'function') {
        try { fn(); } catch(e) {}
    }
    return __timer_id;
};
window.clearTimeout = function(id) {};
window.setInterval = function(fn, delay) {
    __timer_id++;
    return __timer_id;
};
window.clearInterval = function(id) {};
window.requestAnimationFrame = function(fn) {
    __timer_id++;
    return __timer_id;
};
window.cancelAnimationFrame = function(id) {};

// -- History / Storage stubs -------------------------------------------------
window.history = {
    pushState: function(){},
    replaceState: function(){},
    go: function(){},
    back: function(){},
    forward: function(){}
};
window.localStorage = {
    _data: {},
    getItem: function(k) { return this._data[k] || null; },
    setItem: function(k, v) { this._data[k] = String(v); },
    removeItem: function(k) { delete this._data[k]; },
    clear: function() { this._data = {}; }
};
window.sessionStorage = window.localStorage;

// -- Events ------------------------------------------------------------------
window.addEventListener = function(type, handler, options) {};
window.removeEventListener = function(type, handler) {};
window.dispatchEvent = function(event) { return true; };

function Event(type, init) {
    this.type = type;
    this.bubbles = (init && init.bubbles) || false;
    this.cancelable = (init && init.cancelable) || false;
    this.preventDefault = function() {};
    this.stopPropagation = function() {};
    this.stopImmediatePropagation = function() {};
    this.target = null;
    this.currentTarget = null;
}
function CustomEvent(type, init) {
    Event.call(this, type, init);
    this.detail = (init && init.detail) || null;
}
window.Event = Event;
window.CustomEvent = CustomEvent;

// -- fetch() stub ------------------------------------------------------------
window.fetch = function(url, options) {
    var promise = {
        _body: '',
        _ok: false,
        then: function(onFulfilled) {
            var resp = {
                ok: this._ok,
                status: 0,
                headers: { get: function(n) { return null; } },
                _body: this._body,
                text: function() {
                    return {
                        then: function(cb) { if (typeof cb === 'function') { try { cb(''); } catch(e) {} } return this; },
                        catch: function() { return this; }
                    };
                },
                json: function() {
                    return {
                        then: function(cb) { if (typeof cb === 'function') { try { cb({}); } catch(e) {} } return this; },
                        catch: function() { return this; }
                    };
                }
            };
            if (typeof onFulfilled === 'function') {
                try { onFulfilled(resp); } catch(e) {}
            }
            return this;
        },
        catch: function(fn) { return this; },
        finally: function(fn) { if (typeof fn === 'function') { try { fn(); } catch(e) {} } return this; }
    };
    return promise;
};

// -- XMLHttpRequest stub -----------------------------------------------------
function XMLHttpRequest() {
    this.readyState = 0;
    this.status = 0;
    this.responseText = '';
    this.response = '';
    this.onreadystatechange = null;
    this.onload = null;
    this.onerror = null;
    this.open = function(method, url, async) {};
    this.send = function(data) {
        this.readyState = 4;
        this.status = 200;
        if (typeof this.onreadystatechange === 'function') {
            try { this.onreadystatechange(); } catch(e) {}
        }
        if (typeof this.onload === 'function') {
            try { this.onload(); } catch(e) {}
        }
    };
    this.setRequestHeader = function(k, v) {};
    this.getResponseHeader = function(k) { return null; };
    this.abort = function() {};
    this.addEventListener = function(type, handler) {
        if (type === 'load') this.onload = handler;
        if (type === 'error') this.onerror = handler;
    };
}
window.XMLHttpRequest = XMLHttpRequest;

// -- Observer stubs ----------------------------------------------------------
function MutationObserver(cb) {
    this.observe = function(target, options) {};
    this.disconnect = function() {};
    this.takeRecords = function() { return []; };
}
window.MutationObserver = MutationObserver;

function IntersectionObserver(cb, opts) {
    this.observe = function(target) {};
    this.unobserve = function(target) {};
    this.disconnect = function() {};
}
window.IntersectionObserver = IntersectionObserver;

function ResizeObserver(cb) {
    this.observe = function(target) {};
    this.unobserve = function(target) {};
    this.disconnect = function() {};
}
window.ResizeObserver = ResizeObserver;

// -- Misc globals ------------------------------------------------------------
window.scrollX = 0;
window.scrollY = 0;
window.innerWidth = 800;
window.innerHeight = 600;
window.devicePixelRatio = 1;
window.screen = { width: 1920, height: 1080, availWidth: 1920, availHeight: 1080 };
window.performance = {
    now: function() { return 0; },
    mark: function() {},
    measure: function() {}
};
window.crypto = {
    getRandomValues: function(arr) { return arr; },
    subtle: {}
};
window.matchMedia = function(q) {
    return { matches: false, media: q, addEventListener: function() {}, removeEventListener: function() {} };
};

// -- Minimal Promise polyfill (if needed) ------------------------------------
if (typeof Promise === 'undefined') {
    function Promise(executor) {
        this._value = undefined;
        this._state = 'pending';
        var self = this;
        var resolve = function(v) { self._state = 'fulfilled'; self._value = v; };
        var reject = function(r) { self._state = 'rejected'; self._value = r; };
        try { executor(resolve, reject); } catch(e) { reject(e); }
    }
    Promise.prototype.then = function(onFulfilled) {
        if (this._state === 'fulfilled' && typeof onFulfilled === 'function') {
            try { onFulfilled(this._value); } catch(e) {}
        }
        return this;
    };
    Promise.prototype.catch = function() { return this; };
    Promise.prototype.finally = function(fn) {
        if (typeof fn === 'function') { try { fn(); } catch(e) {} }
        return this;
    };
    Promise.resolve = function(v) { return new Promise(function(res) { res(v); }); };
    Promise.reject = function(r) { return new Promise(function(_, rej) { rej(r); }); };
    Promise.all = function(arr) { return Promise.resolve(arr); };
    Promise.allSettled = function(arr) {
        return Promise.resolve(arr.map(function(v) { return { status: 'fulfilled', value: v }; }));
    };
    window.Promise = Promise;
}
