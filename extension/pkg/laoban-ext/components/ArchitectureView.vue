<script lang="ts">
// 3D view of a plan's architecture: three layer planes (network, code, data),
// one mesh per component, curved lines for connections. Orbit and zoom;
// clicking a component selects it, and with `editable` a component can be
// dragged along its layer plane (the page saves the new positions as a
// version). Ported from web/viewer-prototype/index.html (three.js 0.160.0).
//
// The selected component gets a halo in the palette's select colour, its
// connections take that colour, and components not connected to it fade.
// The toolbar (top right) has the palette picker (🎨), "reset view", and
// a `tools` slot for the page's own buttons (enlarge, full screen).
//
// Names and labels come from AI output: they are put on screen with
// textContent only, never innerHTML.
import {
  defineComponent, onBeforeUnmount, onMounted, ref, watch, computed, nextTick, PropType
} from 'vue';
import { useStore } from 'vuex';
import * as THREE from 'three';
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js';
import { CSS2DRenderer, CSS2DObject } from 'three/examples/jsm/renderers/CSS2DRenderer.js';
import { useText } from '../composables/useText';
import { useAppearance } from '../composables/useAppearance';
import { PALETTES } from '../config/appearance';
import { relatedNodes } from '../utils/layout';
import {
  layoutArchitecture, withPositions, LAYERS, LAYER_Y, KIND_HEIGHT, SceneLayout, Positions
} from '../utils/layout';
import { edgeKey, DiffStatus, DiffSummary } from '../utils/graphDiff';

// Version comparison (utils/graphDiff.ts mergeForDiff): status per node id
// and per edge key; the architecture prop is then the merged graph.
export interface DiffMarks {
  nodes: Record<string, DiffStatus>;
  edges: Record<string, DiffStatus>;
  summary: DiffSummary;
}

const DIFF_PREFIX: Partial<Record<DiffStatus, string>> = {
  added: '＋ ', removed: '－ ', changed: '～ '
};

const GEOM: Record<string, () => THREE.BufferGeometry> = {
  ingress:  () => new THREE.OctahedronGeometry(0.75),
  service:  () => new THREE.CylinderGeometry(0.55, 0.55, 0.5, 24),
  netpol:   () => new THREE.TorusGeometry(0.6, 0.12, 10, 32).rotateX(Math.PI / 2),
  app:      () => new THREE.BoxGeometry(1.6, 0.8, 1.1),
  module:   () => new THREE.BoxGeometry(0.9, 0.45, 0.7),
  database: () => new THREE.CylinderGeometry(0.85, 0.85, 1.1, 32),
  table:    () => new THREE.BoxGeometry(1.0, 0.3, 0.75),
  queue:    () => new THREE.CapsuleGeometry(0.35, 0.9, 6, 16).rotateZ(Math.PI / 2),
  external: () => new THREE.DodecahedronGeometry(0.6),
  storage:  () => new THREE.CylinderGeometry(0.9, 0.9, 0.35, 6),
};

// The seven kinds of the phase-1 3D view, for the legend.
export const COMPONENT_KINDS = ['app', 'service', 'database', 'ingress', 'external', 'storage', 'queue'] as const;

// Pointer travel (px) below which a press is a click, not a drag.
const CLICK_SLOP = 4;
const MAX_COORD = 100;

const COLOR_TOKENS = ['layer-network', 'layer-code', 'layer-data', 'edge', 'select', 'diff-added', 'diff-removed', 'diff-changed'];

// Opacity of components and connections that are not related to the
// selected one.
const FADED = 0.22;

function disposeObject(root: THREE.Object3D) {
  root.traverse((obj) => {
    const o = obj as THREE.Object3D & {
      geometry?: THREE.BufferGeometry;
      material?: THREE.Material | THREE.Material[];
    };

    o.geometry?.dispose();
    if (o.material) {
      (Array.isArray(o.material) ? o.material : [o.material]).forEach((m) => m.dispose());
    }
    if (obj instanceof CSS2DObject) {
      obj.element.remove();
    }
  });
  root.clear();
}

function makeLabel(textValue: string, cls: string, badge = 0): CSS2DObject {
  const el = document.createElement('div');

  el.className = cls;
  el.textContent = textValue; // never innerHTML: names may come from AI output
  if (badge > 0) {
    const b = document.createElement('span');

    b.className = 'laoban-opinion-badge';
    b.textContent = `💬 ${ badge }`;
    el.appendChild(b);
  }

  return new CSS2DObject(el);
}

export default defineComponent({
  name: 'LaobanArchitectureView',

  props: {
    architecture: {
      type:    Object as PropType<unknown>,
      default: null,
    },
    diff: {
      type:    Object as PropType<DiffMarks | null>,
      default: null,
    },
    // Unsaved dragged positions, by node id.
    positions: {
      type:    Object as PropType<Positions | null>,
      default: null,
    },
    // Components can be dragged (latest version only).
    editable: {
      type:    Boolean,
      default: false,
    },
    selected: {
      type:    String as PropType<string | null>,
      default: null,
    },
    // Opinions in the running round, by node id: shown next to the name.
    badges: {
      type:    Object as PropType<Record<string, number> | null>,
      default: null,
    },
  },

  emits: ['move', 'select'],

  setup(props, { emit }) {
    const store = useStore();
    const { t } = useText(store);
    const { palette, setPalette } = useAppearance();
    const paletteOpen = ref(false);
    const paletteButton = ref<HTMLElement | null>(null);
    const paletteMenu = ref<HTMLElement | null>(null);

    const root = ref<HTMLElement | null>(null);
    const stage = ref<HTMLElement | null>(null);
    const labels = ref<HTMLElement | null>(null);
    const webglFailed = ref(false);

    const layout = computed<SceneLayout>(() => layoutArchitecture(withPositions(props.architecture, props.positions)));

    // three.js objects live outside Vue reactivity.
    let renderer: THREE.WebGLRenderer | null = null;
    let labelRenderer: CSS2DRenderer | null = null;
    let scene: THREE.Scene | null = null;
    let camera: THREE.PerspectiveCamera | null = null;
    let controls: OrbitControls | null = null;

    function onWheel(ev: WheelEvent) {
      if (controls) {
        controls.enableZoom = ev.ctrlKey || ev.metaKey;
      }
    }
    let world: THREE.Group | null = null;
    let resizeObserver: ResizeObserver | null = null;
    let themeObserver: MutationObserver | null = null;
    let reduceMotion = false;
    let renderQueued = false;
    // Meshes that can be picked, by node id.
    let pickable: THREE.Mesh[] = [];
    const raycaster = new THREE.Raycaster();
    let drag: { mesh: THREE.Mesh; id: string; plane: THREE.Plane; startX: number; startY: number; moved: boolean; pointerId: number } | null = null;

    function render() {
      if (renderer && labelRenderer && scene && camera) {
        renderer.render(scene, camera);
        labelRenderer.render(scene, camera);
      }
    }

    // With reduced motion there is no animation loop: draw only when
    // something changes.
    function requestRender() {
      if (!reduceMotion || renderQueued) {
        return;
      }
      renderQueued = true;
      requestAnimationFrame(() => {
        renderQueued = false;
        render();
      });
    }

    function readColors(): Record<string, THREE.Color> {
      const cs = root.value ? getComputedStyle(root.value) : null;

      return Object.fromEntries(COLOR_TOKENS.map((n) => {
        const v = cs?.getPropertyValue(`--laoban-${ n }`).trim();

        // Fallback if the view is used outside a laoban page.
        return [n, new THREE.Color(v || '#888888')];
      }));
    }

    function frameCamera(l: SceneLayout) {
      if (!camera || !controls) {
        return;
      }
      const span = Math.max(l.plane.width, l.plane.depth);
      const dist = Math.max(22, span * 1.4);

      controls.target.set(l.plane.centerX, 3.5, l.plane.centerZ);
      camera.position.set(l.plane.centerX + dist * 0.6, 12 + span * 0.15, l.plane.centerZ + dist * 0.75);
      controls.maxDistance = Math.max(60, dist * 2.5);
      controls.update();
    }

    /** Back to the starting angle and distance. */
    function resetView() {
      frameCamera(layout.value);
      requestRender();
    }

    function choosePalette(id: string) {
      setPalette(id);
      paletteOpen.value = false;
      paletteButton.value?.focus();
    }

    function onDocPointer(ev: PointerEvent) {
      const target = ev.target as Node | null;

      if (paletteOpen.value && target && !paletteMenu.value?.contains(target) && !paletteButton.value?.contains(target)) {
        paletteOpen.value = false;
      }
    }

    function onPaletteKey(ev: KeyboardEvent) {
      if (ev.key === 'Escape') {
        // Close the menu only; the enlarged view stays open.
        ev.stopPropagation();
        paletteOpen.value = false;
        paletteButton.value?.focus();
      }
    }

    function togglePalette() {
      paletteOpen.value = !paletteOpen.value;
      if (paletteOpen.value) {
        nextTick(() => {
          paletteMenu.value?.querySelector<HTMLElement>('[aria-checked="true"]')?.focus();
        });
      }
    }

    function build(reframe = true) {
      if (!world) {
        return;
      }
      disposeObject(world);
      pickable = [];
      const l = layout.value;
      const colors = readColors();
      const {
        width, depth, centerX, centerZ
      } = l.plane;

      for (const layer of LAYERS) {
        const y = LAYER_Y[layer];
        const color = colors[`layer-${ layer }`];
        const plane = new THREE.Mesh(
          new THREE.PlaneGeometry(width, depth).rotateX(-Math.PI / 2),
          new THREE.MeshBasicMaterial({
            color, transparent: true, opacity: 0.07, depthWrite: false, side: THREE.DoubleSide
          }),
        );

        plane.position.set(centerX, y, centerZ);
        world.add(plane);
        const border = new THREE.LineSegments(
          new THREE.EdgesGeometry(new THREE.PlaneGeometry(width, depth).rotateX(-Math.PI / 2)),
          new THREE.LineBasicMaterial({
            color, transparent: true, opacity: 0.5
          }),
        );

        border.position.set(centerX, y, centerZ);
        world.add(border);
        const lbl = makeLabel(t(`laoban.dashboard.arch.layers.${ layer }`), 'laoban-layer-label');

        lbl.position.set(centerX + width / 2 - 0.6, y + 0.05, centerZ + depth / 2 - 0.4);
        world.add(lbl);
      }

      const positions = new Map<string, THREE.Vector3>();

      const diff = props.diff;
      const selected = props.selected && l.nodes.some((n) => n.id === props.selected) ? props.selected : null;
      const related = selected ? relatedNodes(l, selected) : null;

      for (const n of l.nodes) {
        const status = diff?.nodes[n.id];
        const material = new THREE.MeshStandardMaterial({
          color: colors[`layer-${ n.layer }`], roughness: 0.55, metalness: 0.05
        });

        if (status === 'added' || status === 'changed') {
          material.emissive = colors[`diff-${ status }`];
          material.emissiveIntensity = 0.55;
        } else if (status === 'removed') {
          material.color = colors['diff-removed'];
          material.transparent = true;
          material.opacity = 0.3;
        }
        if (related && !related.has(n.id)) {
          material.transparent = true;
          material.opacity = Math.min(material.opacity, FADED);
        }
        const geometry = (GEOM[n.kind] || GEOM.module)();
        const mesh = new THREE.Mesh(geometry, material);

        mesh.position.set(n.x, n.y, n.z);
        mesh.userData.nodeId = n.id;
        world.add(mesh);
        pickable.push(mesh);
        positions.set(n.id, mesh.position.clone());

        const cls = ['laoban-node-label'];

        if (status) {
          cls.push(`laoban-diff-${ status }`);
        }
        if (selected === n.id) {
          cls.push('laoban-selected');
          material.emissive = colors.select;
          material.emissiveIntensity = 0.45;
          // Halo: the same shape, a little larger, drawn from the inside.
          const halo = new THREE.Mesh(geometry.clone(), new THREE.MeshBasicMaterial({
            color: colors.select, side: THREE.BackSide, transparent: true, opacity: 0.85, depthWrite: false
          }));

          halo.scale.setScalar(1.22);
          halo.raycast = () => undefined;
          mesh.add(halo);
        } else if (related && !related.has(n.id)) {
          cls.push('laoban-faded');
        }
        const lbl = makeLabel(`${ (status && DIFF_PREFIX[status]) || '' }${ n.name }`, cls.join(' '), props.badges?.[n.id] || 0);

        lbl.position.set(0, (KIND_HEIGHT[n.kind] ?? KIND_HEIGHT.module) + 0.2, 0);
        mesh.add(lbl);
      }

      for (const e of l.edges) {
        const p0 = positions.get(e.from);
        const p2 = positions.get(e.to);

        if (!p0 || !p2) {
          continue;
        }
        const mid = p0.clone().add(p2).multiplyScalar(0.5);

        if (e.sameLayer) {
          mid.y += 1.2;
        }
        const curve = new THREE.QuadraticBezierCurve3(p0, mid, p2);
        const estatus = diff?.edges[edgeKey(e)];
        const touches = !!selected && (e.from === selected || e.to === selected);
        let ecolor = estatus === 'added' || estatus === 'removed' || estatus === 'changed' ? colors[`diff-${ estatus }`] : colors.edge;
        let eopacity = estatus === 'removed' ? 0.35 : 0.9;

        if (touches) {
          ecolor = colors.select;
          eopacity = 1;
        } else if (selected) {
          eopacity = Math.min(eopacity, FADED);
        }
        const line = new THREE.Line(
          new THREE.BufferGeometry().setFromPoints(curve.getPoints(32)),
          new THREE.LineBasicMaterial({
            color: ecolor, transparent: true, opacity: eopacity
          }),
        );

        world.add(line);
        // direction marker at 75%
        const tip = new THREE.Mesh(new THREE.SphereGeometry(touches ? 0.13 : 0.09, 10, 8), new THREE.MeshBasicMaterial({
          color: ecolor, transparent: true, opacity: eopacity
        }));

        tip.position.copy(curve.getPoint(0.75));
        world.add(tip);
        if (e.label) {
          const lbl = makeLabel(e.label, `laoban-edge-label${ touches ? ' laoban-edge-selected' : selected ? ' laoban-faded' : '' }`);

          lbl.position.copy(curve.getPoint(0.5));
          world.add(lbl);
        }
      }

      if (reframe) {
        frameCamera(l);
      }
      requestRender();
    }

    function pointerRay(ev: PointerEvent): boolean {
      if (!renderer || !camera) {
        return false;
      }
      const r = renderer.domElement.getBoundingClientRect();

      if (!r.width || !r.height) {
        return false;
      }
      raycaster.setFromCamera(new THREE.Vector2(((ev.clientX - r.left) / r.width) * 2 - 1, -((ev.clientY - r.top) / r.height) * 2 + 1), camera);

      return true;
    }

    function onPointerDown(ev: PointerEvent) {
      if (ev.button !== 0 || !pointerRay(ev)) {
        return;
      }
      const hit = raycaster.intersectObjects(pickable, false)[0];

      if (!hit) {
        return;
      }
      const mesh = hit.object as THREE.Mesh;

      drag = {
        mesh,
        id:        String(mesh.userData.nodeId),
        plane:     new THREE.Plane(new THREE.Vector3(0, 1, 0), -mesh.position.y),
        startX:    ev.clientX,
        startY:    ev.clientY,
        moved:     false,
        pointerId: ev.pointerId,
      };
      if (props.editable && controls) {
        // Dragging a component, not the camera.
        controls.enabled = false;
        renderer?.domElement.setPointerCapture?.(ev.pointerId);
      }
    }

    function onPointerMove(ev: PointerEvent) {
      if (!drag || ev.pointerId !== drag.pointerId) {
        return;
      }
      if (!drag.moved && Math.hypot(ev.clientX - drag.startX, ev.clientY - drag.startY) < CLICK_SLOP) {
        return;
      }
      drag.moved = true;
      if (!props.editable || !pointerRay(ev)) {
        return;
      }
      const p = new THREE.Vector3();

      if (raycaster.ray.intersectPlane(drag.plane, p)) {
        drag.mesh.position.x = Math.max(-MAX_COORD, Math.min(MAX_COORD, p.x));
        drag.mesh.position.z = Math.max(-MAX_COORD, Math.min(MAX_COORD, p.z));
        requestRender();
      }
    }

    function onPointerUp(ev: PointerEvent) {
      if (!drag || ev.pointerId !== drag.pointerId) {
        return;
      }
      const d = drag;

      drag = null;
      if (controls) {
        controls.enabled = true;
      }
      if (!d.moved) {
        emit('select', d.id === props.selected ? null : d.id);
      } else if (props.editable) {
        const round = (v: number) => Math.round(v * 10) / 10;

        emit('move', d.id, { x: round(d.mesh.position.x), z: round(d.mesh.position.z) });
      }
    }

    function resize() {
      if (!stage.value || !renderer || !labelRenderer || !camera) {
        return;
      }
      const r = stage.value.getBoundingClientRect();
      const w = Math.max(1, Math.floor(r.width));
      const h = Math.max(1, Math.floor(r.height));

      renderer.setSize(w, h);
      labelRenderer.setSize(w, h);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      requestRender();
    }

    onMounted(() => {
      if (!stage.value || !labels.value) {
        return;
      }
      try {
        renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
      } catch (e) {
        webglFailed.value = true;

        return;
      }
      renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
      stage.value.prepend(renderer.domElement);
      // Before OrbitControls' own listeners, so a drag on a component can
      // turn the camera off for that gesture.
      renderer.domElement.addEventListener('pointerdown', onPointerDown);
      // The wheel scrolls the page; Ctrl/⌘ + wheel (and trackpad pinch,
      // which the browser sends as Ctrl + wheel) zooms. Registered before
      // OrbitControls' own wheel listener.
      renderer.domElement.addEventListener('wheel', onWheel, { passive: true });
      renderer.domElement.addEventListener('pointermove', onPointerMove);
      renderer.domElement.addEventListener('pointerup', onPointerUp);
      renderer.domElement.addEventListener('pointercancel', onPointerUp);
      labelRenderer = new CSS2DRenderer({ element: labels.value });

      scene = new THREE.Scene();
      camera = new THREE.PerspectiveCamera(42, 1, 0.1, 400);
      controls = new OrbitControls(camera, renderer.domElement);
      reduceMotion = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches || false;
      controls.enableDamping = !reduceMotion;
      controls.enablePan = true;
      controls.maxPolarAngle = Math.PI * 0.49;
      controls.minDistance = 6;

      scene.add(new THREE.AmbientLight(0xffffff, 0.75));
      const sun = new THREE.DirectionalLight(0xffffff, 0.9);

      sun.position.set(8, 20, 10);
      scene.add(sun);
      world = new THREE.Group();
      scene.add(world);

      resizeObserver = new ResizeObserver(resize);
      resizeObserver.observe(stage.value);
      // Rancher switches themes with a class on <body>; re-read the colors.
      themeObserver = new MutationObserver(() => build(false));
      themeObserver.observe(document.body, { attributes: true, attributeFilter: ['class'] });
      document.addEventListener('pointerdown', onDocPointer, true);

      resize();
      build();

      if (reduceMotion) {
        controls.addEventListener('change', requestRender);
        render();
      } else {
        renderer.setAnimationLoop(() => {
          controls?.update();
          render();
        });
      }
    });

    watch(() => props.architecture, () => build());
    // Same graph, other marks, positions or selection: keep the camera.
    watch(() => [props.diff, props.positions, props.selected, props.badges], () => build(false), { deep: true });
    // The palette is an attribute on the root: re-read the colors once the
    // DOM has it.
    watch(palette, () => nextTick(() => build(false)));

    onBeforeUnmount(() => {
      resizeObserver?.disconnect();
      themeObserver?.disconnect();
      document.removeEventListener('pointerdown', onDocPointer, true);
      renderer?.setAnimationLoop(null);
      controls?.removeEventListener('change', requestRender);
      if (renderer) {
        renderer.domElement.removeEventListener('pointerdown', onPointerDown);
        renderer.domElement.removeEventListener('wheel', onWheel);
        renderer.domElement.removeEventListener('pointermove', onPointerMove);
        renderer.domElement.removeEventListener('pointerup', onPointerUp);
        renderer.domElement.removeEventListener('pointercancel', onPointerUp);
      }
      controls?.dispose();
      if (world) {
        disposeObject(world);
      }
      scene?.clear();
      if (renderer) {
        renderer.dispose();
        renderer.domElement.remove();
      }
      labels.value?.replaceChildren();
      renderer = null;
      labelRenderer = null;
      scene = null;
      camera = null;
      controls = null;
      world = null;
    });

    return {
      t,
      root,
      stage,
      labels,
      layout,
      webglFailed,
      layers:   LAYERS,
      kinds:    COMPONENT_KINDS,
      palettes: PALETTES,
      palette,
      paletteOpen,
      paletteButton,
      paletteMenu,
      choosePalette,
      togglePalette,
      onPaletteKey,
      resetView,
    };
  },
});
</script>

<template>
  <div
    ref="root"
    class="laoban-arch"
    :data-laoban-palette="palette"
  >
    <div class="laoban-arch-stage-wrap">
    <div
      ref="stage"
      class="laoban-arch-stage"
      role="img"
      :aria-label="t('laoban.dashboard.arch.ariaLabel', { count: layout.nodes.length })"
    >
      <div
        ref="labels"
        class="laoban-arch-labels"
      />
      <p
        v-if="webglFailed"
        class="laoban-arch-fallback text-muted"
      >
        {{ t('laoban.dashboard.arch.noWebgl') }}
      </p>
    </div>
    <div class="laoban-arch-tools">
      <slot name="tools" />
      <button
        type="button"
        class="btn btn-sm role-secondary"
        :disabled="webglFailed"
        @click="resetView"
      >
        {{ t('laoban.view3d.resetView') }}
      </button>
      <span class="laoban-palette">
        <button
          ref="paletteButton"
          type="button"
          class="btn btn-sm role-secondary"
          aria-haspopup="true"
          :aria-expanded="paletteOpen ? 'true' : 'false'"
          :title="t('laoban.view3d.palette')"
          :aria-label="t('laoban.view3d.palette')"
          @click="togglePalette"
        >
          🎨
        </button>
        <div
          v-if="paletteOpen"
          ref="paletteMenu"
          class="laoban-palette-menu"
          role="radiogroup"
          :aria-label="t('laoban.view3d.palette')"
          @keydown="onPaletteKey"
        >
          <span class="laoban-palette-title">{{ t('laoban.view3d.palette') }}</span>
          <button
            v-for="p in palettes"
            :key="p"
            type="button"
            role="radio"
            class="laoban-palette-option"
            :class="{ selected: palette === p }"
            :aria-checked="palette === p ? 'true' : 'false'"
            @click="choosePalette(p)"
          >
            <span
              class="laoban-palette-swatch"
              :data-laoban-palette="p"
              aria-hidden="true"
            ><i /><i /><i /><b /></span>
            {{ t(`laoban.view3d.palettes.${ p }`) }}
          </button>
          <span class="text-muted laoban-palette-note">{{ t('laoban.view3d.paletteNote') }}</span>
        </div>
      </span>
    </div>
    </div>
    <div class="laoban-arch-legend">
      <span
        v-for="layer in layers"
        :key="layer"
      >
        <i
          :class="`swatch-${ layer }`"
          aria-hidden="true"
        />{{ t(`laoban.dashboard.arch.layers.${ layer }`) }}
      </span>
      <template v-if="diff">
        <span
          v-for="s in ['added', 'removed', 'changed']"
          :key="s"
          data-testid="laoban-diff-legend"
        >
          <i
            :class="`swatch-diff-${ s }`"
            aria-hidden="true"
          />{{ t(`laoban.diff.${ s }`, { count: diff.summary[s] }) }}
        </span>
      </template>
      <span v-if="selected">
        <i
          class="swatch-select"
          aria-hidden="true"
        />{{ t('laoban.view3d.selected') }}
      </span>
      <span class="text-muted">{{ editable ? t('laoban.layout.dragHint') : t('laoban.dashboard.arch.hint') }}</span>
    </div>
    <details class="laoban-arch-list">
      <summary>{{ t('laoban.dashboard.arch.listTitle', { count: layout.nodes.length }) }}</summary>
      <ul>
        <li
          v-for="n in layout.nodes"
          :key="n.id"
        >
          {{ t(`laoban.dashboard.arch.layers.${ n.layer }`) }} · {{ t(`laoban.kinds.${ n.kind }`) }} · {{ n.name }}
        </li>
      </ul>
    </details>
    <details class="laoban-arch-list">
      <summary>{{ t('laoban.kinds.title') }}</summary>
      <ul>
        <li
          v-for="k in kinds"
          :key="k"
        >
          {{ t(`laoban.kinds.${ k }`) }} — {{ t(`laoban.kinds.shape.${ k }`) }}
        </li>
      </ul>
    </details>
  </div>
</template>

<style lang="scss" scoped>
// Layer and select colours (--laoban-layer-*, --laoban-edge,
// --laoban-select) come from the view's palette (styles/palettes.scss) and
// follow Rancher's light/dark theme.
.laoban-arch {
  display: flex;
  flex-direction: column;
  gap: 8px;
  height: 100%;
  min-height: 0;
}

.laoban-arch-stage-wrap {
  position: relative;
  display: flex;
  flex-direction: column;
  flex: 1 1 auto;
  min-height: 0;
}

// Toolbar over the top-right corner of the view.
.laoban-arch-tools {
  position: absolute;
  top: 8px;
  right: 8px;
  z-index: 2;
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  gap: 6px;
  max-width: calc(100% - 16px);
}

.laoban-palette {
  position: relative;
}

.laoban-palette-menu {
  position: absolute;
  top: calc(100% + 4px);
  right: 0;
  z-index: 3;
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 200px;
  padding: 8px;
  border: 1px solid var(--border);
  border-radius: var(--border-radius);
  background: var(--body-bg);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
}

.laoban-palette-title {
  font-weight: bold;
  font-size: 12px;
  margin-bottom: 4px;
}

.laoban-palette-note {
  font-size: 11px;
  margin-top: 4px;
}

.laoban-palette-option {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 6px;
  border: 1px solid transparent;
  border-radius: var(--border-radius);
  background: none;
  color: var(--body-text);
  text-align: left;
  cursor: pointer;

  &:hover {
    background: var(--sortable-table-hover-bg, var(--box-bg));
  }

  &.selected {
    border-color: var(--primary);
    font-weight: bold;
  }
}

// Three layer stripes and the select dot of the palette it stands for.
.laoban-palette-swatch {
  display: inline-flex;
  align-items: center;
  gap: 1px;

  i {
    width: 12px;
    height: 14px;
  }

  i:nth-child(1) { background: var(--laoban-layer-network); }
  i:nth-child(2) { background: var(--laoban-layer-code); }
  i:nth-child(3) { background: var(--laoban-layer-data); }

  b {
    width: 10px;
    height: 10px;
    margin-left: 4px;
    border-radius: 50%;
    background: var(--laoban-select);
  }
}

.laoban-arch-stage {
  position: relative;
  flex: 1 1 auto;
  min-height: 360px;
  overflow: hidden;
  border-radius: var(--border-radius);
  background: var(--sortable-table-bg, var(--body-bg));

  :deep(canvas) {
    display: block;
    touch-action: none;
  }
}

.laoban-arch-labels {
  position: absolute;
  inset: 0;
  pointer-events: none;

  :deep(.laoban-node-label) {
    font-size: 12px;
    padding: 1px 6px;
    border-radius: 3px;
    background: var(--body-bg);
    border: 1px solid var(--border);
    color: var(--body-text);
    white-space: nowrap;
    transform: translateY(-50%);
  }

  // Version comparison marks on labels (utils/graphDiff.ts).
  :deep(.laoban-diff-added) {
    border-color: var(--laoban-diff-added);
    box-shadow: inset 3px 0 0 var(--laoban-diff-added);
  }

  :deep(.laoban-diff-changed) {
    border-color: var(--laoban-diff-changed);
    box-shadow: inset 3px 0 0 var(--laoban-diff-changed);
  }

  :deep(.laoban-diff-removed) {
    border-color: var(--laoban-diff-removed);
    color: var(--muted);
    text-decoration: line-through;
  }

  :deep(.laoban-selected) {
    border: 2px solid var(--laoban-select);
    box-shadow: 0 0 0 2px var(--laoban-select);
    font-weight: bold;
  }

  :deep(.laoban-faded) {
    opacity: 0.4;
  }

  :deep(.laoban-edge-selected) {
    color: var(--laoban-select);
    font-weight: bold;
  }

  :deep(.laoban-opinion-badge) {
    margin-left: 6px;
    padding: 0 5px;
    border-radius: 8px;
    background: var(--primary);
    color: var(--primary-text, #fff);
    font-size: 11px;
    font-weight: normal;
  }

  :deep(.laoban-edge-label) {
    font-size: 11px;
    color: var(--muted);
    white-space: nowrap;
  }

  :deep(.laoban-layer-label) {
    font-size: 11px;
    letter-spacing: 0.04em;
    color: var(--muted);
  }
}

.laoban-arch-fallback {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 16px;
  text-align: center;
}

.laoban-arch-legend {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 14px;
  font-size: 12px;

  span {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }

  i {
    display: inline-block;
    width: 9px;
    height: 9px;
    border-radius: 2px;
  }

  .swatch-diff-added { background: var(--laoban-diff-added); }
  .swatch-diff-removed { background: var(--laoban-diff-removed); }
  .swatch-diff-changed { background: var(--laoban-diff-changed); }
  .swatch-network { background: var(--laoban-layer-network); }
  .swatch-code { background: var(--laoban-layer-code); }
  .swatch-data { background: var(--laoban-layer-data); }
  .swatch-select { background: var(--laoban-select); }
}

.laoban-arch-list {
  font-size: 12px;

  ul {
    margin: 4px 0 0;
    padding-left: 18px;
  }

  li {
    overflow-wrap: anywhere;
  }
}
</style>
