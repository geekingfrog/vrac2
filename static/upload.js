"use strict";

// the ids/names for the files, merely used to avoid collision
let counter = 0;

const removeRow = target => ev => {
  console.log("clicked", ev);
  target.parentNode.removeChild(target);
}

const loadImagePreview = inputElement => {
  let imgEl = document.createElement("img");
  imgEl.src = "//:0";
  imgEl.style.display = "none";
  inputElement.insertAdjacentElement("afterend", imgEl);

  let vidEl = document.createElement("video");
  vidEl.controls = true;
  vidEl.width = "500";
  vidEl.height = "400";
  vidEl.style.display = "none";
  inputElement.insertAdjacentElement("afterend", vidEl);
  window.vidEl = vidEl;

  inputElement.addEventListener("change", (_ev) => {
    if (inputElement.files && inputElement.files[0]) {
      let file = inputElement.files[0];
      console.log(`mimetype is: ${file.type}`);
      window.f = inputElement.files[0];
      if (file.type.startsWith("image")) {
        imgEl.alt = `preview for ${inputElement.name}`;
        imgEl.src = URL.createObjectURL(file);
        imgEl.style.display = "block";
        imgEl.onload = () => {
          console.log("revoking url for image");
          URL.revokeObjectURL(imgEl.src);
        }
      } else {
        imgEl.src = "//:0";
        imgEl.alt = "";
        imgEl.style.display = "none";
      }

      if (file.type.startsWith("video")) {
        let vidSrc = document.createElement("source");
        vidSrc.source = "//:0";
        vidEl.insertAdjacentElement("beforeEnd", vidSrc);
        window.vidSrc = vidSrc;

        vidEl.style.display = "block";
        let u = URL.createObjectURL(file);
        vidSrc.src = u;
        vidSrc.type = file.type;
        // no onload=>revokeObjectURL since it doesn't seem to work (?)
      } else if (vidEl.children.length > 0) {
        vidEl.removeChild(vidSrc);
        vidSrc.src = "//:0";
        vidEl.style.display = "none";
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
  inputElement.multiple = true;

  p.insertAdjacentElement("afterbegin", inputElement);
  loadImagePreview(inputElement);
  let delButton = document.createElement("button");
  delButton.setAttribute("type", "button");
  delButton.innerHTML = "❌";
  p.insertAdjacentElement("beforeend", delButton);
  delButton.addEventListener("click", removeRow(p), {"once": true});
  return p;
}

const addRow = (endEl) => (_ev) => {
  counter++;
  let el = addFile(counter);
  endEl.insertAdjacentElement("beforebegin", el);
}

window.onload = function onload() {
  let p = document.createElement("p");
  let button = document.createElement("button");
  button.type = "button";
  button.innerText = "Add file";
  p.insertAdjacentElement("afterbegin", button);
  document.querySelector("#upload-form").insertAdjacentElement("afterbegin", p);

  let p2 = p.cloneNode(true);
  document.querySelector("#upload-form [type='submit']").insertAdjacentElement("beforebegin", p2);

  let addRowInForm = addRow(p2);
  p.addEventListener("click", addRowInForm);
  p2.addEventListener("click", addRowInForm);

  addRowInForm();
}
