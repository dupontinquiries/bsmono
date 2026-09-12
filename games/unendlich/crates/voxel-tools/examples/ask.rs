fn main() {
    println!("== the prompt a model sees\n{}\n", voxel_tools::probe::prompt_lines());
    println!("== answers");
    for q in [r#"{"q":"block","name":"water"}"#, r#"{"q":"block","name":"sand"}"#,
              r#"{"q":"try","block":"sand","on":"air"}"#, r#"{"q":"try","block":"water","on":"stone"}"#,
              r#"{"q":"try","block":"stone","on":"stone"}"#, r#"{"q":"for","want":"something that glows"}"#,
              r#"{"q":"like","of":"deepslate"}"#, r#"{"q":"block","name":"amethyst"}"#] {
        println!("   {q}");
        println!("     {}", voxel_tools::probe::answer(q));
    }
}
