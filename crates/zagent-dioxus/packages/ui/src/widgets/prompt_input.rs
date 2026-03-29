use dioxus::prelude::*;

use crate::models::PromptImagePreviewView;

#[derive(Props, Clone, PartialEq)]
pub struct PromptInputProps {
    pub value: String,
    pub pending: bool,
    pub recording: bool,
    pub transcribing: bool,
    pub image_previews: Vec<PromptImagePreviewView>,
    pub on_input: EventHandler<String>,
    pub on_submit: EventHandler<()>,
    pub on_copy: Option<EventHandler<()>>,
    pub on_paste: Option<EventHandler<()>>,
    pub on_native_paste: Option<EventHandler<ClipboardEvent>>,
    pub on_pick_images: Option<EventHandler<()>>,
    pub on_remove_image: EventHandler<usize>,
    pub on_toggle_recording: Option<EventHandler<()>>,
    pub children: Element,
}

#[component]
pub fn PromptInput(props: PromptInputProps) -> Element {
    let input_locked = props.pending || props.transcribing;
    let submit = {
        let on_submit = props.on_submit.clone();
        move |_| on_submit.call(())
    };

    rsx! {
        div { class: "prompt-shell",
            div { class: "prompt-main",
                if !props.image_previews.is_empty() {
                    div { class: "prompt-image-strip",
                        for preview in props.image_previews {
                            div { class: "prompt-image-chip",
                                img {
                                    class: "prompt-image-preview",
                                    src: "{preview.data_url}",
                                    alt: "{preview.name}"
                                }
                                div { class: "prompt-image-meta",
                                    span { class: "prompt-image-name", "{preview.name}" }
                                    button {
                                        class: "prompt-image-remove",
                                        onclick: {
                                            let on_remove = props.on_remove_image.clone();
                                            let id = preview.id;
                                            move |_| on_remove.call(id)
                                        },
                                        disabled: input_locked,
                                        title: "Remove image",
                                        aria_label: "Remove image",
                                        {icon_x()}
                                    }
                                }
                            }
                        }
                    }
                }
                textarea {
                    class: "prompt-input",
                    id: "prompt-composer-input",
                    value: "{props.value}",
                    placeholder: if props.pending {
                        "Assistant is running..."
                    } else if props.transcribing {
                        "Voice transcription in progress..."
                    } else if props.recording {
                        "Recording voice note..."
                    } else {
                        "Message zAgent"
                    },
                    disabled: input_locked,
                    rows: 4,
                    oninput: {
                        let on_input = props.on_input.clone();
                        move |evt| on_input.call(evt.value())
                    },
                    onkeydown: {
                        let on_submit = props.on_submit.clone();
                        move |evt| {
                            if evt.key() == Key::Enter && !evt.modifiers().shift() {
                                evt.prevent_default();
                                on_submit.call(());
                            }
                        }
                    },
                    onpaste: move |evt| {
                        if let Some(on_native_paste) = props.on_native_paste.clone() {
                            on_native_paste.call(evt);
                        }
                    }
                }
                div { class: "prompt-toolbar",
                    div { class: "prompt-toolbar-group",
                        if let Some(on_copy) = props.on_copy.clone() {
                            button {
                                class: "prompt-tool-button",
                                disabled: input_locked,
                                onclick: move |_| on_copy.call(()),
                                title: "Copy prompt",
                                aria_label: "Copy prompt",
                                {icon_copy()}
                                span { class: "prompt-tool-label", "Copy" }
                            }
                        }
                        if let Some(on_paste) = props.on_paste.clone() {
                            button {
                                class: "prompt-tool-button",
                                disabled: input_locked,
                                onclick: move |_| on_paste.call(()),
                                title: "Paste text or images",
                                aria_label: "Paste text or images",
                                {icon_clipboard()}
                                span { class: "prompt-tool-label", "Paste" }
                            }
                        }
                        if let Some(on_pick_images) = props.on_pick_images.clone() {
                            button {
                                class: "prompt-tool-button",
                                disabled: input_locked,
                                onclick: move |_| on_pick_images.call(()),
                                title: "Attach images",
                                aria_label: "Attach images",
                                {icon_image()}
                                span { class: "prompt-tool-label", "Images" }
                            }
                        }
                    }
                    div { class: "prompt-toolbar-group prompt-toolbar-group-end",
                        if let Some(on_toggle_recording) = props.on_toggle_recording.clone() {
                            button {
                                class: if props.recording {
                                    "prompt-tool-button recording"
                                } else if props.transcribing {
                                    "prompt-tool-button busy"
                                } else {
                                    "prompt-tool-button"
                                },
                                disabled: props.pending || props.transcribing,
                                onclick: move |_| on_toggle_recording.call(()),
                                title: if props.recording { "Stop recording" } else { "Record voice note" },
                                aria_label: if props.recording { "Stop recording" } else { "Record voice note" },
                                {icon_mic()}
                                span {
                                    class: "prompt-tool-label",
                                    if props.recording { "Stop" } else if props.transcribing { "Working" } else { "Voice" }
                                }
                            }
                        }
                        button {
                            class: "send-button",
                            disabled: input_locked,
                            onclick: submit,
                            title: "Send message",
                            aria_label: "Send message",
                            {icon_send()}
                            span { class: "prompt-tool-label", if props.pending { "Running" } else { "Send" } }
                        }
                    }
                }
            }
            div { class: "prompt-slot", {props.children} }
        }
    }
}

fn icon_copy() -> Element {
    rsx! {
        svg {
            class: "prompt-tool-icon",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.8",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            rect { x: "9", y: "9", width: "10", height: "10", rx: "2" }
            path { d: "M6 15H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h8a2 2 0 0 1 2 2v1" }
        }
    }
}

fn icon_clipboard() -> Element {
    rsx! {
        svg {
            class: "prompt-tool-icon",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.8",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            rect { x: "6", y: "4", width: "12", height: "16", rx: "2" }
            path { d: "M9 4.5h6" }
            path { d: "M9 9h6" }
            path { d: "M9 13h6" }
        }
    }
}

fn icon_image() -> Element {
    rsx! {
        svg {
            class: "prompt-tool-icon",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.8",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            rect { x: "3", y: "5", width: "18", height: "14", rx: "2" }
            circle { cx: "9", cy: "10", r: "1.6" }
            path { d: "m21 16-5.2-5.2a1 1 0 0 0-1.4 0L8 17" }
        }
    }
}

fn icon_mic() -> Element {
    rsx! {
        svg {
            class: "prompt-tool-icon",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.8",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            rect { x: "9", y: "3", width: "6", height: "11", rx: "3" }
            path { d: "M5 10a7 7 0 0 0 14 0" }
            path { d: "M12 17v4" }
            path { d: "M8.5 21h7" }
        }
    }
}

fn icon_send() -> Element {
    rsx! {
        svg {
            class: "prompt-tool-icon",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.8",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M4 12 20 4l-4 16-3.5-5.5z" }
        }
    }
}

fn icon_x() -> Element {
    rsx! {
        svg {
            class: "prompt-tool-icon prompt-tool-icon-small",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M6 6 18 18" }
            path { d: "M18 6 6 18" }
        }
    }
}
