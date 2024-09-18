from typing import Optional, List, Tuple, Any
from prompt_toolkit import HTML
from prompt_toolkit.shortcuts import print_formatted_text
from prompt_toolkit.styles import Style
from prompt_toolkit.formatted_text import PygmentsTokens
from pygments.token import Token
from pygments.lexers import guess_lexer_for_filename
import pygments
import shutil

Tokens = List[Tuple[Any, str]]
Lines = List[Tokens]


def get_terminal_width() -> int:
    return shutil.get_terminal_size((80, 20))[0]


def split_newline_tokens(tokens: Tokens) -> List[Tokens]:
    result = []
    for token in tokens:
        first = True
        for line in token[1].split("\n"):
            if not first:
                result.append((token[0], "\n"))
            result.append((token[0], line))
            if first:
                first = False
    return result


def wrap_tokens(tokens: Tokens, max_width: int) -> Lines:
    tokens = split_newline_tokens(tokens)
    result = []
    current_line = []
    line_length = 0
    for token in tokens:
        token_len = len(token[1])
        if token_len + line_length > max_width:
            result.append(current_line)
            current_line = []
            line_length = 0
        while token_len > max_width:
            result.append((token[0], token[1][:max_width]))
            token = (token[0], token[1][max_width:])
            token_len = len(token[1])

        if token[1] == "\n":
            result.append(current_line)
            current_line = []
            line_length = 0
        elif token_len != 0:
            current_line.append(token)
            line_length += token_len
    return result


def wrap_text(text: str, max_width: int) -> Lines:
    lines = text.split("\n")
    result = []
    for line in lines:
        last_whitespace = 0
        start = 0
        for i in range(len(line)):
            if i - start >= max_width:
                if last_whitespace <= start:
                    last_whitespace = i - 1
                result.append(to_tokens(line[start:last_whitespace + 1]))
                start = last_whitespace + 1
            if line[i].isspace():
                last_whitespace = i
        result.append(to_tokens(line[start:]))
    return result


def indent(lines: Lines, amount: int) -> List[str]:
    return [to_tokens(" " * amount) + line for line in lines]


def to_tokens(text: str) -> Tokens:
    return [(Token.Text, text)]


def tokens_len(tokens: Tokens) -> int:
    return sum([len(x[1]) for x in tokens])


def create_box(
    text: str,
    max_width: int,
    max_height: Optional[int] = None,
    title: Optional[str] = None,
    file_name: Optional[str] = None
) -> Lines:
    if file_name is not None:
        lexer = guess_lexer_for_filename(file_name, text)
        tokens = list(pygments.lex(text, lexer=lexer))
        lines = wrap_tokens(tokens, max_width - 2)
    else:
        lines = wrap_text(text, max_width - 2)

    result = []

    if title is None:
        result.append(to_tokens("┌" + "─" * (max_width - 2) + "┐"))
    else:
        title_len = min(len(title), max_width - 6)
        bar_len = max_width - 5 - title_len
        result.append(
            to_tokens("┌─ " + title[:title_len] + " " + "─" * bar_len + "┐"))

    if max_height is not None and len(lines) > max_height:
        lines = lines[0:max_height - 1]
        lines.append(to_tokens("..."))

    for line in lines:
        line_len = tokens_len(line)
        space_len = max_width - line_len - 2
        new_line = to_tokens("│") + line + to_tokens(" " * space_len + "│")
        result.append(new_line)
    result.append(to_tokens("└" + "─" * (max_width - 2) + "┘"))
    return result


def print_header(text: str, width: int) -> str:
    style = Style.from_dict({
        'block': 'bg:ansiwhite fg:ansiblack',
    })
    text_width = len(text)
    left = (width - text_width - 2) // 2
    right = width - text_width - 2 - left
    print_formatted_text(HTML("─" * left + "<block> " +
                         text + " </block>" + "─" * right), style=style)