// Start the app: load the wasm (Rust from here on) and register the service worker for an
// offline start. The Content-Security-Policy allows no inline script, so this is its own file.
import init from './app.js';

const loading = document.getElementById('rg-loading');
try {
    if (!navigator.gpu) throw new Error('Ravenglass needs a browser with WebGPU (current Chrome, Edge or Safari; Firefox 141+).');
    await init();
    loading?.remove();
} catch (error) {
    if (loading) loading.textContent = String(error?.message ?? error);
    console.error(error);
}

if ('serviceWorker' in navigator && location.protocol !== 'file:') {
    navigator.serviceWorker.register('sw.js', { scope: './' }).catch((error) => console.warn('service worker:', error));
}
