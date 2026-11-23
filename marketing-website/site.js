const menuToggle = document.querySelector("[data-menu-toggle]");

if (menuToggle) {
  const nav = document.getElementById(menuToggle.getAttribute("aria-controls"));
  const narrowScreen = window.matchMedia("(max-width: 50rem)");

  function setMenuOpen(open, restoreFocus = false) {
    nav.hidden = narrowScreen.matches && !open;
    menuToggle.setAttribute("aria-expanded", String(open));
    if (restoreFocus) menuToggle.focus();
  }

  function syncMenu() {
    menuToggle.hidden = !narrowScreen.matches;
    setMenuOpen(false, narrowScreen.matches && nav.contains(document.activeElement));
  }

  menuToggle.closest(".nav-bar").setAttribute("data-enhanced", "");
  syncMenu();
  narrowScreen.addEventListener("change", syncMenu);

  if (window.ResizeObserver) {
    const header = menuToggle.closest(".site-header");
    const updateScrollPadding = () => {
      document.documentElement.style.setProperty("--header-height", `${header.offsetHeight}px`);
    };
    new ResizeObserver(updateScrollPadding).observe(header);
    updateScrollPadding();
  }

  menuToggle.addEventListener("click", () => setMenuOpen(nav.hidden));

  nav.addEventListener("click", (event) => {
    const link = event.target.closest("a");
    if (!narrowScreen.matches || !link) return;

    setMenuOpen(false);
    if (link.hash && link.getAttribute("href").startsWith("#")) {
      document.getElementById(link.hash.slice(1))?.focus({ preventScroll: true });
    }
  });

  document.addEventListener("click", (event) => {
    if (menuToggle.getAttribute("aria-expanded") !== "true") return;
    if (!nav.contains(event.target) && !menuToggle.contains(event.target)) {
      setMenuOpen(false, nav.contains(document.activeElement));
    }
  });

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && menuToggle.getAttribute("aria-expanded") === "true") {
      setMenuOpen(false, true);
    }
  });

  const sectionLinks = nav.querySelectorAll('a[href^="#"]');
  function markCurrentSection() {
    sectionLinks.forEach((link) => {
      if (link.hash === window.location.hash) link.setAttribute("aria-current", "location");
      else link.removeAttribute("aria-current");
    });
  }

  markCurrentSection();
  window.addEventListener("hashchange", markCurrentSection);
}

document.querySelectorAll("[data-tabs]").forEach((group) => {
  const list = group.querySelector("[data-tab-list]");
  const tabs = Array.from(list.querySelectorAll("[data-panel]"));
  const panels = tabs.map((tab) => document.getElementById(tab.dataset.panel));

  function selectTab(index, moveFocus = false) {
    tabs.forEach((tab, position) => {
      const selected = position === index;
      tab.setAttribute("aria-selected", String(selected));
      tab.tabIndex = selected ? 0 : -1;
      panels[position].hidden = !selected;
    });
    if (moveFocus) tabs[index].focus();
  }

  tabs.forEach((tab, index) => {
    tab.setAttribute("role", "tab");
    tab.setAttribute("aria-controls", panels[index].id);
    panels[index].setAttribute("role", "tabpanel");
    panels[index].setAttribute("aria-labelledby", tab.id);
    panels[index].tabIndex = 0;

    tab.addEventListener("click", () => selectTab(index));
    tab.addEventListener("keydown", (event) => {
      const last = tabs.length - 1;
      let next;
      switch (event.key) {
        case "ArrowDown":
        case "ArrowRight": next = index === last ? 0 : index + 1; break;
        case "ArrowUp":
        case "ArrowLeft": next = index === 0 ? last : index - 1; break;
        case "Home": next = 0; break;
        case "End": next = last; break;
        default: return;
      }
      event.preventDefault();
      selectTab(next, true);
    });
  });

  selectTab(0);
  list.setAttribute("role", "tablist");
  list.setAttribute("aria-label", list.dataset.tabList);
  list.hidden = false;
  group.setAttribute("data-enhanced", "");
});

const copyStatus = document.getElementById("copy-status");

if (navigator.clipboard?.writeText) {
  document.querySelectorAll("[data-copy]").forEach((button) => {
    button.hidden = false;
    const command = document.getElementById(button.dataset.copy);

    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(command.textContent.trim());
        button.textContent = "Copied";
        button.dataset.copied = "";
        copyStatus.textContent = "Command copied to clipboard.";
      } catch {
        const selection = window.getSelection();
        selection?.selectAllChildren(command);
        button.textContent = "Copy";
        delete button.dataset.copied;
        copyStatus.textContent = selection
          ? "Clipboard access was unavailable. The command is selected for manual copying."
          : "Clipboard access was unavailable. Copy the command manually.";
      }
    });
    button.addEventListener("blur", () => {
      button.textContent = "Copy";
      delete button.dataset.copied;
      copyStatus.textContent = "";
    });
  });
}
