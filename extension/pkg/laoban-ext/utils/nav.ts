// Rancher's side menu (Overview, Apps) can be hidden on laoban's pages, to
// leave the room to the work (Sam, after report 21). Rancher lays its
// page out as a grid whose menu column is `--nav-width`; a class on <body>
// sets it to 0 and hides the menu (styles/global.scss). The class is only
// there while a laoban page is shown, so the rest of Rancher keeps its
// menu. The choice is kept in this browser.
import { ref } from 'vue';

const KEY = 'laoban-nav-collapsed';
const CLASS = 'laoban-nav-collapsed';

function read(): boolean {
  try {
    return window.localStorage.getItem(KEY) === '1';
  } catch {
    return false;
  }
}

export const navCollapsed = ref(read());

function apply(on: boolean) {
  try {
    document.body.classList.toggle(CLASS, on);
  } catch {}
}

export function setNavCollapsed(on: boolean) {
  navCollapsed.value = on;
  apply(on);
  try {
    window.localStorage.setItem(KEY, on ? '1' : '0');
  } catch {}
}

/** A laoban page is shown: use the kept choice. */
export function enterPage() {
  apply(navCollapsed.value);
}

/** Leaving laoban's pages: Rancher's menu comes back. */
export function leavePage() {
  apply(false);
}
