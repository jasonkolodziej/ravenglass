// The service worker (architecture §8): keeps the app shell for an offline start. Mail never
// passes through here; requests to the Relay and anything not in the shell go to the network.
// tools/web-build.sh fills in the build's version and file list.
const CACHE = 'ravenglass-{{VERSION}}';
const SHELL = {{SHELL}};

self.addEventListener('install', (event) => {
    event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(SHELL)).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (event) => {
    event.waitUntil(caches.keys()
        .then((keys) => Promise.all(keys.filter((key) => key.startsWith('ravenglass-') && key !== CACHE).map((key) => caches.delete(key))))
        .then(() => self.clients.claim()));
});

// Network first, so a new build is picked up at once; the cached shell when offline.
self.addEventListener('fetch', (event) => {
    const url = new URL(event.request.url);
    if (event.request.method !== 'GET' || url.origin !== self.location.origin) return;
    const path = url.pathname.slice(new URL(self.registration.scope).pathname.length) || 'index.html';
    if (!SHELL.includes(path) && event.request.mode !== 'navigate') return;
    event.respondWith(fetch(event.request)
        .then((response) => {
            if (response.ok) { const copy = response.clone(); caches.open(CACHE).then((cache) => cache.put(event.request, copy)); }
            return response;
        })
        .catch(() => caches.match(event.request).then((hit) => hit ?? caches.match('index.html'))));
});
