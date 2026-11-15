package app.warpdrop.android.ui.components

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowForward
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.OffsetMapping
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.input.TransformedText
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.warpdrop.android.ui.theme.Warp

/** Renders `482910` as `482-910` while the model keeps digits only. */
object CodeDashTransformation : VisualTransformation {
    override fun filter(text: AnnotatedString): TransformedText {
        val d = text.text
        val out = if (d.length > 3) d.substring(0, 3) + "-" + d.substring(3) else d
        val mapping = object : OffsetMapping {
            override fun originalToTransformed(offset: Int): Int = if (offset > 3) offset + 1 else offset
            override fun transformedToOriginal(offset: Int): Int =
                (if (offset > 3) offset - 1 else offset).coerceIn(0, d.length)
        }
        return TransformedText(AnnotatedString(out), mapping)
    }
}

/**
 * Six-digit code entry with an auto dash. Accepts pastes of `482910`,
 * `482-910` or a `warpdrop://…` link — [onChange] receives the raw text and
 * the caller normalizes it.
 */
@Composable
fun CodeField(
    digits: String,
    onChange: (String) -> Unit,
    onSubmit: () -> Unit,
    label: String,
    modifier: Modifier = Modifier,
    submitDescription: String = label,
) {
    val c = Warp.colors
    var focused by remember { mutableStateOf(false) }
    val complete = digits.length == 6
    Row(
        modifier
            .fillMaxWidth()
            .height(48.dp)
            .clip(Warp.controlShape)
            .background(c.surfaceHover)
            .border(if (focused) 1.5.dp else 1.dp, if (focused) c.accent else c.border, Warp.controlShape)
            .padding(start = 12.dp, end = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.weight(1f)) {
            if (digits.isEmpty()) {
                Text("000-000", style = Warp.codeStyle.copy(fontSize = 20.sp), color = c.textTertiary)
            }
            BasicTextField(
                value = TextFieldValue(digits, selection = TextRange(digits.length)),
                onValueChange = { onChange(it.text) },
                singleLine = true,
                textStyle = Warp.codeStyle.copy(fontSize = 20.sp, color = c.text),
                cursorBrush = SolidColor(c.accent),
                visualTransformation = CodeDashTransformation,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Go),
                keyboardActions = KeyboardActions(onGo = { if (complete) onSubmit() }),
                modifier = Modifier
                    .fillMaxWidth()
                    .onFocusChanged { focused = it.isFocused }
                    .semantics { contentDescription = label },
            )
        }
        Spacer(Modifier.width(4.dp))
        ToolButton(
            icon = Icons.AutoMirrored.Rounded.ArrowForward,
            contentDescription = submitDescription,
            onClick = onSubmit,
            enabled = complete,
            tint = if (complete) c.accent else c.textTertiary,
        )
    }
}
