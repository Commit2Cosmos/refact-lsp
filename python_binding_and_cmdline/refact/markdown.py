from refact.printing import Tokens, Lines

gray = "#252b37"

def is_special_boundary(char: str) -> bool:
    return char in "*_[](){}:.,;!?-"

def is_word_boundary(text: str, i: int, after: bool = False, l: int = 1) -> bool:
    if after:
        return i + l >= len(text) or text[i+l].isspace() or is_special_boundary(text[i+l])
    else:
        return i - l < 0 or text[i-1].isspace() or is_special_boundary(text[i-1])

def to_markdown(text: str, width: int) -> Tokens:
    result = []
    last = -1
    i = 0

    is_bold = False
    is_italic = False
    is_inline_code = False

    def get_format():
        res = []
        if is_bold:
            res.append("bold")
        if is_italic:
            res.append("italic")
        if is_inline_code:
            res.append(f"bg:{gray}")
        return " ".join(res)


    while i < len(text):


        # `text`
        if text[i] == "`" and text[i+1] != "`":
            result.append((get_format(), text[last + 1:i]))
            if is_inline_code:
                result.append((gray, ""))
            else:
                result.append((gray, ""))
            last = i
            is_inline_code = not is_inline_code

        # skip all backticks
        elif text[i] == "`":
            while text[i] == "`":
                i += 1

        # *italic text*
        elif text[i] == "*" and text[i+1] != "*" and is_word_boundary(text, i, is_italic):
            result.append((get_format(), text[last + 1:i]))
            last = i
            is_italic = not is_italic

        # _italic text_
        elif text[i] == "_" and text[i+1] != "_" and is_word_boundary(text, i, is_italic):
            result.append((get_format(), text[last + 1:i]))
            last = i
            is_italic = not is_italic

        # **bold text**
        elif text[i:i+2] == "**" and is_word_boundary(text, i, is_bold, 2):
            result.append((get_format(), text[last + 1:i]))
            i += 1
            last = i
            is_bold = not is_bold

        # __bold text__
        elif text[i:i+2] == "__" and is_word_boundary(text, i, is_bold, 2):
            result.append((get_format(), text[last + 1:i]))
            i += 1
            last = i
            is_bold = not is_bold

        i += 1

    result.append(("", text[last + 1:]))
    return result
