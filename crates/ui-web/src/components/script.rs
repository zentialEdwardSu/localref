//! Small browser interaction script for client-side filtering and routing.

/// Inline JavaScript for local, no-refresh filtering, routing, and events.
pub(super) const INTERACTION_SCRIPT: &str = r#"
document.addEventListener('DOMContentLoaded', () => {
  const routeClickTimers = new WeakMap();
  let filterTimer = 0;
  const syncHidden = (search, category) => {
    document.querySelectorAll('[data-filter-q]').forEach((input) => input.value = search?.value || '');
    document.querySelectorAll('[data-filter-category]').forEach((input) => input.value = category?.value || '');
  };
  const filterRouteFrom = () => {
    const routeUrl = new URL(window.location.href);
    const search = document.getElementById('library-search');
    const category = document.getElementById('library-category');
    const q = (search?.value || '').trim();
    const selectedCategory = category?.value || '';
    if (q) {
      routeUrl.searchParams.set('q', q);
    } else {
      routeUrl.searchParams.delete('q');
    }
    if (selectedCategory) {
      routeUrl.searchParams.set('category', selectedCategory);
    } else {
      routeUrl.searchParams.delete('category');
    }
    routeUrl.searchParams.delete('selected');
    return routeUrl;
  };
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
  const applyFilters = () => {
    const search = document.getElementById('library-search');
    const category = document.getElementById('library-category');
    syncHidden(search, category);
  };
  const routeUrlFrom = (button) => {
    const routeUrl = new URL(window.location.href);
    if (button.dataset.routeActive) {
      routeUrl.searchParams.set('active', button.dataset.routeActive);
      routeUrl.searchParams.delete('selected');
    }
    if (button.dataset.routeTab) {
      routeUrl.searchParams.set('tab', button.dataset.routeTab);
    }
    return routeUrl;
  };
  const selectionRouteFrom = (form) => {
    const routeUrl = new URL(form.action || '/', window.location.href);
    const formData = new FormData(form);
    const selected = formData.getAll('item').filter(Boolean);
    formData.delete('item');
    const params = new URLSearchParams(formData);
    params.delete('selected');
    if (selected.length > 0) {
      params.set('selected', selected.join(','));
    }
    routeUrl.search = params.toString();
    return routeUrl;
  };
  const visitRoute = async (routeUrl, pushHistory) => {
    const response = await fetch(routeUrl, {
      headers: { 'X-Localref-UI-Router': '1' },
    });
    await replaceShell(response, routeUrl, pushHistory);
  };
  const scheduleFilterRoute = () => {
    window.clearTimeout(filterTimer);
    filterTimer = window.setTimeout(() => {
      visitRoute(filterRouteFrom(), true);
    }, 250);
  };
  const submitAction = async (form) => {
    const actionUrl = form.getAttribute('action') || form.action;
    const keepCategoryEditorOpen = form.closest('.category-editor')?.open ?? false;
    const response = await fetch(actionUrl, {
      method: form.method || 'POST',
      body: new URLSearchParams(new FormData(form)),
      headers: { 'X-Localref-UI-Router': '1' },
    });
    await replaceShell(response, new URL(response.url), true);
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
  const replaceShell = async (response, routeUrl, pushHistory) => {
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
    if (pushHistory) {
      history.pushState({}, '', routeUrl);
    }
    bindPage();
  };
  const bindPage = () => {
    const search = document.getElementById('library-search');
    const category = document.getElementById('library-category');
    const eventsToggle = document.querySelector('[data-events-toggle]');
    if (search && !search.dataset.filterBound) {
      search.dataset.filterBound = 'true';
      search.addEventListener('input', scheduleFilterRoute);
      search.addEventListener('change', () => visitRoute(filterRouteFrom(), true));
      search.addEventListener('keydown', (event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          visitRoute(filterRouteFrom(), true);
        }
      });
    }
    if (category && !category.dataset.filterBound) {
      category.dataset.filterBound = 'true';
      category.addEventListener('change', () => visitRoute(filterRouteFrom(), true));
    }
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
    document.querySelectorAll('[data-route-tab],[data-route-active]').forEach((button) => {
      if (button.dataset.routeBound) {
        return;
      }
      button.dataset.routeBound = 'true';
      button.addEventListener('click', () => {
        const timer = window.setTimeout(() => {
          routeClickTimers.delete(button);
          visitRoute(routeUrlFrom(button), true);
        }, 220);
        routeClickTimers.set(button, timer);
      });
      button.addEventListener('dblclick', () => {
        const timer = routeClickTimers.get(button);
        if (timer) {
          window.clearTimeout(timer);
          routeClickTimers.delete(button);
        }
        openItemFile(button);
      });
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
    document.querySelectorAll('form.selection-form').forEach((form) => {
      if (form.dataset.selectionBound) {
        return;
      }
      form.dataset.selectionBound = 'true';
      form.addEventListener('submit', (event) => {
        event.preventDefault();
        visitRoute(selectionRouteFrom(form), true);
      });
    });
    document.querySelectorAll('.row-check').forEach((checkbox) => {
      if (checkbox.dataset.selectionBound) {
        return;
      }
      checkbox.dataset.selectionBound = 'true';
      checkbox.addEventListener('change', () => {
        const form = checkbox.form;
        if (form) {
          visitRoute(selectionRouteFrom(form), true);
        }
      });
    });
    applyFilters();
  };
  window.addEventListener('popstate', () => {
    visitRoute(new URL(window.location.href), false);
  });
  bindPage();
});
"#;
