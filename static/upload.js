"use strict";

// the ids/names for the files, merely used to avoid collision
let counter = 0;

const removeRow = target => ev => {
// function removeRow(ev) {
  console.log("clicked", ev);
  target.parentNode.removeChild(target);
}

const loadImagePreview = inputElement => {
  let imgEl = document.createElement("img");
  imgEl.src = "//:0";

  inputElement.insertAdjacentElement("afterend", imgEl);
  inputElement.addEventListener("change", (_ev) => {
    if (inputElement.files && inputElement.files[0]) {
      if ( inputElement.files[0].type.startsWith("image")) {
        imgEl.alt = `preview for ${inputElement.name}`;
        imgEl.src = URL.createObjectURL(inputElement.files[0]);
        imgEl.onload = () => {
          URL.revokeObjectURL(imgEl.src);
        }
      } else {
        imgEl.src = "//:0";
        imgEl.alt = "";
      }
    }
  });

  imgEl.addEventListener("click", (ev) => {
    ev.stopPropagation();
    inputElement.click();
  });
}

function addFile(id) {
  let p = document.createElement("p");
  let inputElement = document.createElement("input");
  inputElement.type = "file";
  inputElement.name = `file_${counter}`;

  p.insertAdjacentElement("afterbegin", inputElement);
  // p.insertAdjacentHTML("afterbegin", `<input type="file" name="file_${counter}">`);
  loadImagePreview(inputElement);
  let closeButton = document.createElement("button");
  closeButton.setAttribute("type", "button");
  closeButton.innerHTML = "close";
  p.insertAdjacentElement("beforeend", closeButton);
  closeButton.addEventListener("click", removeRow(p), {"once": true});
  return p;
}

function addRow(_ev) {
  counter++;
  let el = addFile(counter);
  const button = document.querySelector("#upload-form [type=submit]");
  button.insertAdjacentElement("beforebegin", el);
}

window.onload = function onload() {
  document.getElementById("add-file").addEventListener("click", addRow);
  console.log("coucou upload");
  addRow()
  // const button = document.querySelector("#upload-form [type=submit]");
  // button.insertAdjacentElement("beforebegin", addFile(1));
}
