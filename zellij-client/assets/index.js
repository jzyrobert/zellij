import { initConnectionHandlers } from './connection.js';
import { initAuthentication } from './auth.js';
import { initTerminal } from './terminal.js';
import { setupInputHandlers } from './input.js';
import { initWebSockets } from './websockets.js';

document.addEventListener("DOMContentLoaded", async (event) => {
    initConnectionHandlers();

    // Capture the deep-link path (?path=) before any await — and strip it
    // from the address bar so reload / bookmark / PWA start_url stay clean
    // and never re-trigger the deep link. Falsy / empty values are skipped.
    const rawDeepLinkPath = new URLSearchParams(location.search).get("path");
    const deepLinkPath = rawDeepLinkPath && rawDeepLinkPath.length > 0 ? rawDeepLinkPath : null;
    if (location.search) {
        history.replaceState(null, "", location.pathname);
    }

    const webClientId = await initAuthentication();

    const { term, fitAddon } = initTerminal();
    const sessionName = location.pathname.split("/").pop();

    let sendAnsiKey = (ansiKey) => {
        // This will be replaced by the WebSocket module
    };

    setupInputHandlers(term, sendAnsiKey);

    document.title = sessionName;
    const websockets = initWebSockets(webClientId, sessionName, term, fitAddon, sendAnsiKey, deepLinkPath);
    
    // Update sendAnsiKey to use the actual WebSocket function returned by initWebSockets
    sendAnsiKey = websockets.sendAnsiKey;
    
    // Update the input handlers with the correct sendAnsiKey function
    setupInputHandlers(term, sendAnsiKey);
});
