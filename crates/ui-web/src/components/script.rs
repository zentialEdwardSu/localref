//! Small browser interaction script for actions not yet migrated to WASM.

/// Inline JavaScript for action forms, events, dialogs, and file-open actions.
pub(super) const INTERACTION_SCRIPT: &str = r#"
document.addEventListener('DOMContentLoaded', () => {
  const setEventsOpen = (open) => {
    const eventsToggle = document.querySelector('[data-events-toggle]');
    const primaryDetail = document.querySelector('[data-primary-detail]');
    const eventPanel = document.querySelector('.event-panel');
    if (primaryDetail) {
      primaryDetail.hidden = open;
    }
    if (eventPanel) {
      eventPanel.hidden = !open;
    }
    if (eventsToggle) {
      eventsToggle.classList.toggle('is-active', open);
      eventsToggle.setAttribute('aria-pressed', open ? 'true' : 'false');
    }
  };
  const dismissDialog = (button) => {
    const dialog = button.closest('.rules-result-dialog');
    if (dialog) {
      dialog.remove();
    }
    const routeUrl = new URL(window.location.href);
    routeUrl.searchParams.delete('rules_status');
    routeUrl.searchParams.delete('rules_error');
    history.replaceState({}, '', routeUrl);
  };
  const replaceShell = async (response, routeUrl) => {
    if (!response.ok) {
      throw new Error(`route failed: ${response.status}`);
    }
    const html = await response.text();
    const doc = new DOMParser().parseFromString(html, 'text/html');
    const nextShell = doc.querySelector('.app-shell');
    const currentShell = document.querySelector('.app-shell');
    if (!nextShell || !currentShell) {
      throw new Error('route response missing app shell');
    }
    currentShell.innerHTML = nextShell.innerHTML;
    history.replaceState({}, '', routeUrl);
    bindPage();
  };
  const submitAction = async (form) => {
    const actionUrl = form.getAttribute('action') || form.action;
    const keepCategoryEditorOpen = form.closest('.category-editor')?.open ?? false;
    const response = await fetch(actionUrl, {
      method: form.method || 'POST',
      body: new URLSearchParams(new FormData(form)),
      headers: { 'X-Localref-UI-Router': '1' },
    });
    await replaceShell(response, new URL(response.url));
    if (keepCategoryEditorOpen) {
      document.querySelector('.category-editor')?.setAttribute('open', '');
    }
  };
  const openItemFile = async (button) => {
    const filePath = button.dataset.openFile || '';
    if (!filePath || !button.dataset.routeActive) {
      return;
    }
    const formData = new URLSearchParams();
    formData.set('return_to', window.location.pathname + window.location.search);
    formData.set('action', 'open_file');
    formData.set('item_id', button.dataset.routeActive);
    formData.set('file_path', filePath);
    await fetch('/ui/action', {
      method: 'POST',
      body: formData,
      headers: { 'X-Localref-UI-Router': '1' },
    });
  };
  const bindPage = () => {
    const eventsToggle = document.querySelector('[data-events-toggle]');
    if (eventsToggle && !eventsToggle.dataset.eventsBound) {
      eventsToggle.dataset.eventsBound = 'true';
      eventsToggle.addEventListener('click', () => {
        const eventPanel = document.querySelector('.event-panel');
        setEventsOpen(eventPanel?.hidden ?? true);
      });
    }
    document.querySelectorAll('[data-dismiss-dialog]').forEach((button) => {
      if (button.dataset.dismissBound) {
        return;
      }
      button.dataset.dismissBound = 'true';
      button.addEventListener('click', () => dismissDialog(button));
    });
    document.querySelectorAll('[data-route-active]').forEach((button) => {
      if (button.dataset.fileOpenBound) {
        return;
      }
      button.dataset.fileOpenBound = 'true';
      button.addEventListener('dblclick', () => openItemFile(button));
    });
    document.querySelectorAll('form[data-route-action]').forEach((form) => {
      if (form.dataset.routeBound) {
        return;
      }
      form.dataset.routeBound = 'true';
      form.addEventListener('submit', (event) => {
        event.preventDefault();
        submitAction(form);
      });
    });
  };
  bindPage();
});
"#;
