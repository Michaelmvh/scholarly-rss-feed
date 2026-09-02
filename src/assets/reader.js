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
    const checkboxes = [
      ...picker.querySelectorAll('input[type="checkbox"][name]'),
    ];
    const summary = picker.querySelector("[data-selection-summary]");
    let dirty = false;

    selectAll.closest(".filter-select-all").hidden = false;

    const updateSummary = () => {
      const selected = checkboxes.filter((checkbox) => checkbox.checked);
      selectAll.checked = selected.length === 0;
      summary.textContent =
        selected.length === 0
          ? summary.dataset.emptyLabel || "Any tracked author"
          : selected.length === 1
            ? selected[0].dataset.label
            : `${selected.length} selected`;
    };

    checkboxes.forEach((checkbox) => {
      checkbox.addEventListener("change", () => {
        dirty = true;
        updateSummary();
      });
    });

    selectAll.addEventListener("change", () => {
      if (selectAll.checked) {
        checkboxes.forEach((checkbox) => {
          checkbox.checked = false;
        });
        dirty = true;
        updateSummary();
      } else if (!checkboxes.some((checkbox) => checkbox.checked)) {
        selectAll.checked = true;
      }
    });

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
