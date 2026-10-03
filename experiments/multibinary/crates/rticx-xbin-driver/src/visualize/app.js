/* RTICX multi-binary system view.
 *
 * Consumes the JSON payload embedded in #xbin-view (produced by the Rust
 * `visualize` module) and renders the core boxes, the shared-memory regions
 * with their per-task FIFO buffers, and the interaction arrows. The payload is
 * generic: adding a task category later only changes how the Rust side fills
 * `executes`/`spawns`, not this file.
 */
(function () {
  "use strict";

  var SVG_NS = "http://www.w3.org/2000/svg";
  var COLORS = [
    "#4f8cff", "#ff8c4f", "#3fbf7f", "#b06cff",
    "#ff5d73", "#2fb9c9", "#c9a227", "#8c8c8c"
  ];

  var model = JSON.parse(document.getElementById("xbin-view").textContent);
  var stage = document.getElementById("stage");
  var rail = document.getElementById("rail");
  var regionsEl = document.getElementById("regions");
  var filtersEl = document.getElementById("filters");
  var metaEl = document.getElementById("meta");
  var legendEl = document.getElementById("legend");
  var svg = document.getElementById("arrows");

  function colorOf(coreId) {
    return COLORS[((coreId % COLORS.length) + COLORS.length) % COLORS.length];
  }

  function hex(value) {
    var text = Number(value).toString(16);
    while (text.length < 8) {
      text = "0" + text;
    }
    return "0x" + text;
  }

  function make(tag, className, text) {
    var node = document.createElement(tag);
    if (className) {
      node.className = className;
    }
    if (text !== undefined) {
      node.textContent = text;
    }
    return node;
  }

  // -- header ----------------------------------------------------------------

  metaEl.textContent =
    "generation " + model.generation +
    "  ·  topology " + model.topology_hash +
    "  ·  layout " + model.layout_hash +
    "  ·  " + model.stats.cores + " cores, " +
    model.stats.tasks + " cross-binary tasks, " +
    model.stats.regions + " regions, " +
    model.stats.doorbells + " doorbell lines";

  // -- core boxes ------------------------------------------------------------

  function taskNode(task, isSpawn) {
    var node = make("div", "task " + (isSpawn ? "spawn" : "exec"));
    node.dataset.taskId = String(task.id);
    node.dataset.pair = task.pair;
    node.appendChild(make("span", "task-name", task.name));

    var detail = isSpawn
      ? "spawns → core " + task.peer + " · " + task.input_type
      : "priority " + task.priority + " · capacity " + task.capacity + " · " + task.input_type;
    node.appendChild(make("span", "task-detail", detail));

    if (task.doorbell !== null && task.doorbell !== undefined) {
      node.appendChild(make("span", "task-doorbell", "doorbell " + task.doorbell));
    }

    node.title = isSpawn
      ? task.name + "::cross_spawn(" + task.input_type + ")   [core " + task.peer + "]"
      : task.name + "  (priority " + task.priority + ", capacity " + task.capacity +
        ", input " + task.input_type + ")";
    return node;
  }

  function taskGroup(title, tasks, isSpawn) {
    var group = make("div", "task-group");
    group.appendChild(make("div", "task-group-title", title));
    if (tasks.length === 0) {
      group.appendChild(make("div", "task-detail", isSpawn ? "—" : "no cross-binary receivers"));
      return group;
    }
    for (var i = 0; i < tasks.length; i++) {
      group.appendChild(taskNode(tasks[i], isSpawn));
    }
    return group;
  }

  function coreCard(core) {
    var card = make("div", "core-card");
    card.dataset.core = String(core.id);
    card.style.setProperty("--core-color", colorOf(core.id));

    var head = make("div", "core-head");
    head.appendChild(make("span", "core-title", "Core " + core.id));
    head.appendChild(make("span", "core-sub", core.app + " · local " + core.local_index));
    card.appendChild(head);

    card.appendChild(taskGroup("Executes", core.executes, false));
    card.appendChild(taskGroup("Spawns", core.spawns, true));
    return card;
  }

  for (var c = 0; c < model.cores.length; c++) {
    rail.appendChild(coreCard(model.cores[c]));
  }

  // -- regions ---------------------------------------------------------------

  function bufferNode(region, buffer) {
    var node = make("div", "buffer");
    node.dataset.taskId = String(buffer.task_id);
    node.dataset.pair = buffer.pair;
    node.style.left = (buffer.offset / region.size) * 100 + "%";
    node.style.width = Math.max((buffer.total_bytes / region.size) * 100, 4) + "%";
    node.style.setProperty("--core-color", colorOf(region.source));

    node.appendChild(make("span", "buffer-name",
      buffer.task_name + "/" + buffer.input_type + " (depth=" + buffer.depth + ")"));
    node.appendChild(make("span", "buffer-meta",
      "@" + buffer.offset + " · " + buffer.total_bytes + " B · cap " + buffer.capacity));
    node.title =
      buffer.task_name + "/" + buffer.input_type + "\n" +
      "depth " + buffer.depth + " (capacity " + buffer.capacity + ")\n" +
      "offset " + buffer.offset + " B, " + buffer.total_bytes + " B = 64 B header + " +
      buffer.elem_size + " × " + buffer.depth + "\n" +
      "priority " + buffer.priority +
      (buffer.doorbell !== null && buffer.doorbell !== undefined ? ", doorbell line " + buffer.doorbell : "");
    return node;
  }

  function regionPanel(region) {
    var panel = make("div", "region");
    panel.dataset.region = region.direction;
    panel.dataset.pair = region.pair;
    panel.style.setProperty("--core-color", colorOf(region.source));

    var head = make("div", "region-head");
    var toggle = make("button", "region-toggle", "-");
    toggle.type = "button";
    toggle.setAttribute("aria-expanded", "true");
    toggle.title = "Show or hide the region details";
    toggle.addEventListener("click", function () {
      var collapsed = panel.classList.toggle("collapsed");
      toggle.textContent = collapsed ? "+" : "-";
      toggle.setAttribute("aria-expanded", String(!collapsed));
      scheduleDraw();
    });
    head.appendChild(toggle);
    head.appendChild(make("span", "region-title", "Region " + region.source + " → " + region.target));
    panel.appendChild(head);

    var details = make("div", "region-details");
    details.appendChild(make("div", "region-detail", "base(src): " + hex(region.base_from_source)));
    details.appendChild(make("div", "region-detail", "base(tgt): " + hex(region.base_from_target)));
    details.appendChild(make("div", "region-detail", "size: " + region.size + " B"));
    details.appendChild(make("div", "region-detail",
      "allocated: " + region.used_bytes + " / " + region.size + " B"));
    panel.appendChild(details);

    var bar = make("div", "region-bar");
    var slots = make("div", "region-slots");
    if (region.buffers.length === 0) {
      slots.appendChild(make("span", "region-empty", "no cross-binary FIFOs allocated"));
    } else {
      for (var i = 0; i < region.buffers.length; i++) {
        slots.appendChild(bufferNode(region, region.buffers[i]));
      }
    }
    bar.appendChild(slots);
    panel.appendChild(bar);

    return panel;
  }

  for (var r = 0; r < model.regions.length; r++) {
    regionsEl.appendChild(regionPanel(model.regions[r]));
  }

  // -- interaction filters (pair -> task tree) -------------------------------

  // Group the flat task list under its unordered pair. `model.tasks` is
  // ordered by (source, target, name), so the children keep that order.
  var tasksByPair = {};
  for (var t = 0; t < model.tasks.length; t++) {
    var filterTask = model.tasks[t];
    if (!tasksByPair[filterTask.pair]) {
      tasksByPair[filterTask.pair] = [];
    }
    tasksByPair[filterTask.pair].push(filterTask);
  }

  function syncGroup(group) {
    if (group.children.length === 0) {
      return;
    }
    var checked = 0;
    for (var i = 0; i < group.children.length; i++) {
      if (group.children[i].checked) {
        checked++;
      }
    }
    group.input.checked = checked === group.children.length;
    group.input.indeterminate = checked > 0 && checked < group.children.length;
  }

  function buildFilterGroup(pair) {
    var group = { key: pair.key, input: null, children: [] };

    var wrap = make("div", "filter-group");

    var head = make("label", "filter filter-pair");
    var parent = document.createElement("input");
    parent.type = "checkbox";
    parent.dataset.filterPair = pair.key;
    parent.addEventListener("change", function () {
      for (var i = 0; i < group.children.length; i++) {
        group.children[i].checked = parent.checked;
      }
      syncGroup(group);
      applyFilter();
    });
    head.appendChild(parent);
    head.appendChild(make("span", "filter-pair-name", "Core " + pair.a + " ↔ Core " + pair.b));
    group.input = parent;
    wrap.appendChild(head);

    var list = make("div", "filter-tasks");
    var tasks = tasksByPair[pair.key] || [];
    if (tasks.length === 0) {
      list.appendChild(make("div", "filter-empty", "no cross-binary tasks (region only)"));
    }
    for (var i = 0; i < tasks.length; i++) {
      var task = tasks[i];
      var label = make("label", "filter filter-task");
      var input = document.createElement("input");
      input.type = "checkbox";
      input.dataset.filterTask = String(task.id);
      input.addEventListener("change", function () {
        syncGroup(group);
        applyFilter();
      });
      label.appendChild(input);
      label.appendChild(make("span", "filter-task-name", task.name + "/" + task.input_type));
      label.appendChild(make("span", "filter-task-detail",
        task.source + " → " + task.target + " · prio " + task.priority +
        (task.doorbell !== null && task.doorbell !== undefined ? " · doorbell " + task.doorbell : "")));
      label.title = task.name + "/" + task.input_type + "  (core " + task.source +
        " → core " + task.target + ", priority " + task.priority + ")";
      group.children.push(input);
      list.appendChild(label);
    }
    wrap.appendChild(list);

    return wrap;
  }

  for (var p = 0; p < model.pairs.length; p++) {
    filtersEl.appendChild(buildFilterGroup(model.pairs[p]));
  }

  if (model.pairs.length === 0) {
    filtersEl.appendChild(make("span", "legend-note", "no cross-binary interactions"));
  }

  document.getElementById("clear").addEventListener("click", function () {
    var inputs = filtersEl.querySelectorAll("input");
    for (var i = 0; i < inputs.length; i++) {
      inputs[i].checked = false;
      inputs[i].indeterminate = false;
    }
    applyFilter();
  });

  // The current selection: checked pairs and checked tasks, as plain objects.
  function selection() {
    var pairs = {};
    var tasks = {};
    var pairInputs = filtersEl.querySelectorAll("input[data-filter-pair]");
    for (var i = 0; i < pairInputs.length; i++) {
      if (pairInputs[i].checked) {
        pairs[pairInputs[i].dataset.filterPair] = true;
      }
    }
    var taskInputs = filtersEl.querySelectorAll("input[data-filter-task]");
    for (var j = 0; j < taskInputs.length; j++) {
      if (taskInputs[j].checked) {
        tasks[taskInputs[j].dataset.filterTask] = true;
      }
    }
    return { pairs: pairs, tasks: tasks };
  }

  // Dims every element that is neither in a selected pair nor a selected task;
  // arrows are drawn separately and only for the selected tasks.
  function updateHighlight(sel) {
    var any = Object.keys(sel.pairs).length > 0 || Object.keys(sel.tasks).length > 0;
    stage.classList.toggle("isolated", any);
    var nodes = stage.querySelectorAll("[data-pair]");
    for (var n = 0; n < nodes.length; n++) {
      var node = nodes[n];
      var active = sel.pairs[node.dataset.pair] === true;
      if (!active && node.dataset.taskId !== undefined && sel.tasks[node.dataset.taskId] === true) {
        active = true;
      }
      node.classList.toggle("active", active);
    }
  }

  function applyFilter() {
    scheduleDraw();
  }

  // -- legend ----------------------------------------------------------------

  (function () {
    var wrap = make("div", "legend-items");
    for (var i = 0; i < model.cores.length; i++) {
      var item = make("span", "legend-item");
      var swatch = make("span", "legend-swatch");
      swatch.style.background = colorOf(model.cores[i].id);
      item.appendChild(swatch);
      item.appendChild(make("span", null, "Core " + model.cores[i].id + " (" + model.cores[i].app + ")"));
      wrap.appendChild(item);
    }
    wrap.appendChild(make("span", "legend-note",
      "Arrow path: spawner stub → FIFO buffer in the shared region → receiver task. " +
      "Check a core pair, or a single task, to draw its arrows and isolate its interactions."));
    legendEl.appendChild(wrap);
  })();

  // -- arrows ------------------------------------------------------------------

  function marker(coreId) {
    var mark = document.createElementNS(SVG_NS, "marker");
    mark.setAttribute("id", "xbin-arrow-" + coreId);
    mark.setAttribute("viewBox", "0 0 10 10");
    mark.setAttribute("refX", "9");
    mark.setAttribute("refY", "5");
    mark.setAttribute("markerWidth", "6");
    mark.setAttribute("markerHeight", "6");
    mark.setAttribute("orient", "auto-start-reverse");
    var tip = document.createElementNS(SVG_NS, "path");
    tip.setAttribute("d", "M 0 0 L 10 5 L 0 10 z");
    tip.setAttribute("fill", colorOf(coreId));
    mark.appendChild(tip);
    return mark;
  }

  function arrow(x1, y1, x2, y2, core, pair, taskId, title) {
    var path = document.createElementNS(SVG_NS, "path");
    var bend = Math.max(26, Math.abs(y2 - y1) * 0.45);
    path.setAttribute("d",
      "M " + x1 + " " + y1 +
      " C " + x1 + " " + (y1 + bend) +
      " " + x2 + " " + (y2 - bend) +
      " " + x2 + " " + y2);
    path.setAttribute("fill", "none");
    path.setAttribute("stroke", colorOf(core));
    path.setAttribute("stroke-width", "1.8");
    path.setAttribute("marker-end", "url(#xbin-arrow-" + core + ")");
    path.setAttribute("data-pair", pair);
    path.setAttribute("data-task-id", taskId);
    path.setAttribute("class", "arrow");
    var tip = document.createElementNS(SVG_NS, "title");
    tip.textContent = title;
    path.appendChild(tip);
    return path;
  }

  function rectOf(node, stageRect) {
    var r = node.getBoundingClientRect();
    var x = r.left - stageRect.left;
    var y = r.top - stageRect.top;
    return {
      left: x,
      top: y,
      w: r.width,
      h: r.height,
      cx: x + r.width / 2,
      bottom: y + r.height
    };
  }

  function drawArrows() {
    while (svg.firstChild) {
      svg.removeChild(svg.firstChild);
    }
    svg.setAttribute("width", stage.clientWidth);
    svg.setAttribute("height", stage.clientHeight);

    // Arrows exist only for the selected tasks; with nothing checked the
    // figure stays clean.
    var sel = selection();
    if (Object.keys(sel.tasks).length === 0) {
      updateHighlight(sel);
      return;
    }

    var defs = document.createElementNS(SVG_NS, "defs");
    for (var i = 0; i < model.cores.length; i++) {
      defs.appendChild(marker(model.cores[i].id));
    }
    svg.appendChild(defs);

    var stageRect = stage.getBoundingClientRect();

    for (var r = 0; r < model.regions.length; r++) {
      var region = model.regions[r];
      for (var b = 0; b < region.buffers.length; b++) {
        var buffer = region.buffers[b];
        if (!sel.tasks[buffer.task_id]) {
          continue;
        }
        var bufferNode = stage.querySelector('.buffer[data-task-id="' + buffer.task_id + '"]');
        if (!bufferNode) {
          continue;
        }
        var box = rectOf(bufferNode, stageRect);

        var sourceNode = rail.querySelector(
          '.core-card[data-core="' + region.source + '"] .task[data-task-id="' +
          buffer.task_id + '"].spawn');
        var targetNode = rail.querySelector(
          '.core-card[data-core="' + region.target + '"] .task[data-task-id="' +
          buffer.task_id + '"].exec');

        if (sourceNode) {
          var source = rectOf(sourceNode, stageRect);
          svg.appendChild(arrow(
            source.cx, source.bottom,
            box.left + box.w * 0.3, box.top,
            region.source, region.pair, buffer.task_id,
            buffer.task_name + "::cross_spawn  (core " + region.source + " → region)"));
        }
        if (targetNode) {
          var target = rectOf(targetNode, stageRect);
          svg.appendChild(arrow(
            box.left + box.w * 0.7, box.top,
            target.cx, target.bottom,
            region.source, region.pair, buffer.task_id,
            buffer.task_name + "  (region → core " + region.target +
            ", priority " + buffer.priority +
            (buffer.doorbell !== null && buffer.doorbell !== undefined ? ", doorbell " + buffer.doorbell : "") + ")"));
        }
      }
    }

    updateHighlight(sel);
  }

  var scheduled = false;
  function scheduleDraw() {
    if (scheduled) {
      return;
    }
    scheduled = true;
    requestAnimationFrame(function () {
      scheduled = false;
      drawArrows();
    });
  }

  window.addEventListener("resize", scheduleDraw);
  window.addEventListener("load", scheduleDraw);
  if (window.ResizeObserver) {
    new ResizeObserver(scheduleDraw).observe(stage);
  }
  scheduleDraw();
})();
