use dioxus::prelude::*;

#[derive(Props, Clone, PartialEq)]
pub struct PromptInputProps {
    pub value: String,
    pub pending: bool,
    pub on_input: EventHandler<String>,
    pub on_submit: EventHandler<()>,
    pub children: Element,
}

#[component]
pub fn PromptInput(props: PromptInputProps) -> Element {
    let submit = {
        let on_submit = props.on_submit.clone();
        move |_| on_submit.call(())
    };

    rsx! {
        div { class: "prompt-shell",
            input {
                class: "prompt-input",
                value: "{props.value}",
                placeholder: if props.pending { "Assistant is running..." } else { "Type a prompt and press Enter" },
                disabled: props.pending,
                oninput: {
                    let on_input = props.on_input.clone();
                    move |evt| on_input.call(evt.value())
                },
                onkeydown: {
                    let on_submit = props.on_submit.clone();
                    move |evt| {
                        if evt.key() == Key::Enter {
                            on_submit.call(());
                        }
                    }
                }
            }
            button {
                class: "send-button",
                disabled: props.pending,
                onclick: submit,
                if props.pending { "Running" } else { "Send" }
            }
            div { class: "prompt-slot", {props.children} }
        }
    }
}
