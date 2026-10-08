(() => {
  const form = document.querySelector(".filters");
  if (!form) return;

  form.classList.add("filters-enhanced");

  form.querySelectorAll("[data-auto-submit]").forEach((select) => {
    select.addEventListener("change", () => form.requestSubmit());
  });

  const pickers = form.querySelectorAll(
    "[data-author-picker], [data-multiselect]",
  );

  pickers.forEach((picker) => {
    const selectAll = picker.querySelector("[data-select-all]");
    const authorMode = picker.querySelector('input[name="view_authors"]');
    const authorSelection = picker.querySelector('input[name="view_author_selection"]');
    const checkboxes = [
      ...picker.querySelectorAll('input[type="checkbox"][name]'),
    ];
    const summary = picker.querySelector("[data-selection-summary]");
    let dirty = false;

    const updateAuthorMode = () => {
      if (!authorMode) return;
      authorMode.value = checkboxes.every((checkbox) => checkbox.checked)
        ? "all"
        : "custom";
      if (authorSelection) authorSelection.disabled = true;
    };

    if (selectAll) {
      selectAll.closest(".filter-select-all").hidden = false;
    }

    const updateSummary = () => {
      const selected = checkboxes.filter((checkbox) => checkbox.checked);
      if (selectAll) {
        selectAll.checked = selected.length === checkboxes.length;
        selectAll.indeterminate = selected.length > 0 && !selectAll.checked;
      }
      summary.textContent =
        selectAll?.checked
          ? "All tracked authors"
          : selected.length === 0
            ? summary.dataset.emptyLabel || "No tracked authors"
            : selected.length === 1
              ? selected[0].dataset.label
              : `${selected.length} selected`;
    };

    checkboxes.forEach((checkbox) => {
      checkbox.addEventListener("change", () => {
        if (!selectAll && !checkboxes.some((candidate) => candidate.checked)) {
          checkbox.checked = true;
          return;
        }
        dirty = true;
        updateAuthorMode();
        updateSummary();
      });
    });

    if (selectAll) {
      selectAll.addEventListener("change", () => {
        checkboxes.forEach((checkbox) => {
          checkbox.checked = selectAll.checked;
        });
        dirty = true;
        updateAuthorMode();
        updateSummary();
      });
    }

    updateSummary();

    picker.addEventListener("toggle", () => {
      if (!picker.open && dirty) form.requestSubmit();
    });

    document.addEventListener("click", (event) => {
      if (
        picker.open &&
        !picker.contains(event.target) &&
        !event.target.closest("a")
      ) {
        picker.open = false;
      }
    });

    picker.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && picker.open) {
        picker.open = false;
        picker.querySelector("summary").focus();
      }
    });
  });
})();
