use std::mem::{self, align_of, size_of};

use cranelift::{
    codegen::{
        ir::{types, ArgumentPurpose, MemFlags, StackSlotData, StackSlotKind, UserFuncName, Value},
        Context,
    },
    prelude::{
        settings, AbiParam, Configurable, FunctionBuilder, FunctionBuilderContext, InstBuilder,
        Signature,
    },
};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};

fn main() {
    let mut flag_builder = settings::builder();
    flag_builder.set("use_colocated_libcalls", "false").unwrap();
    flag_builder.set("is_pic", "false").unwrap();

    let isa_builder = cranelift_native::builder().unwrap_or_else(|msg| {
        panic!("host machine is not supported: {}", msg);
    });
    let isa = isa_builder
        .finish(settings::Flags::new(flag_builder))
        .unwrap();

    let builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    let mut module = JITModule::new(builder);

    let mut builder_ctx = FunctionBuilderContext::new();
    let mut ctx = module.make_context();

    let foo = create_foo(&mut module, &mut builder_ctx, &mut ctx);
    let main = create_main(&mut module, &mut builder_ctx, &mut ctx, foo);

    // Finalize the functions which were defined, which resolves any
    // outstanding relocations (patching in addresses, now that they're
    // available).
    // This also prepares the code for JIT execution
    module.finalize_definitions().unwrap();

    let code_ptr = module.get_finalized_function(main);

    let main = unsafe { mem::transmute::<*const u8, fn() -> i32>(code_ptr) };

    let value = main();

    println!("The program worked! Good job!");
    println!("output = {value}");
}

fn create_main(
    module: &mut JITModule,
    builder_ctx: &mut FunctionBuilderContext,
    ctx: &mut Context,
    foo: FuncId,
) -> FuncId {
    // generate main function

    let main_sig = Signature {
        params: vec![],
        returns: vec![AbiParam::new(types::I32)],
        call_conv: module.target_config().default_call_conv,
    };
    let main_id = module
        .declare_function("main", Linkage::Export, &main_sig)
        .unwrap();

    ctx.func.signature = main_sig;
    ctx.func.name = UserFuncName::testcase("main");

    // Create the builder to build a function.
    let mut builder = FunctionBuilder::new(&mut ctx.func, builder_ctx);

    // Create the entry block, to start emitting code in.
    let entry_block = builder.create_block();

    builder.switch_to_block(entry_block);
    // tell the builder that the block will have no further predecessors
    builder.seal_block(entry_block);

    let value = generate_main_body(module, &mut builder, foo);

    builder.ins().return_(&[value]);

    builder.seal_all_blocks();
    builder.finalize();

    println!("{}", ctx.func);

    module
        .define_function(main_id, ctx)
        .expect("error defining function");

    module.clear_context(ctx);

    main_id
}

fn generate_main_body(module: &mut JITModule, builder: &mut FunctionBuilder, foo: FuncId) -> Value {
    let ptr_ty = module.target_config().pointer_type();

    let foo = module.declare_func_in_func(foo, builder.func);

    // a struct with a single i32 member
    let struct_size = 4;
    let struct_align = 4;

    assert_eq!(struct_size, size_of::<i32>() as u32);
    assert_eq!(struct_align, align_of::<i32>() as u32);

    let struct_val = {
        let stack_slot = builder.create_sized_stack_slot(StackSlotData {
            kind: StackSlotKind::ExplicitSlot,
            size: struct_size,
            align_shift: struct_align as u8,
        });

        let member_val = builder.ins().iconst(types::I32, 42);

        builder.ins().stack_store(member_val, stack_slot, 0);

        builder.ins().stack_addr(ptr_ty, stack_slot, 0)
    };

    let call = builder.ins().call(foo, &[struct_val]);
    let result = builder.inst_results(call)[0];

    result
}

fn create_foo(
    module: &mut JITModule,
    builder_ctx: &mut FunctionBuilderContext,
    ctx: &mut Context,
) -> FuncId {
    // generate foo function

    let ptr_ty = module.target_config().pointer_type();

    // the "struct" contains a single i32
    let struct_size = 4;

    // align the struct
    let mask = 16 - 1;
    let aggr_pointee_size = (struct_size + mask) & !mask;

    dbg!(aggr_pointee_size);

    // this is the part that causes the crash
    let struct_param =
        AbiParam::special(ptr_ty, ArgumentPurpose::StructArgument(aggr_pointee_size));
    // uncomment this to make the code work
    // let struct_param = AbiParam::new(ptr_ty);

    let foo_sig = Signature {
        params: vec![struct_param],
        returns: vec![AbiParam::new(types::I32)],
        call_conv: module.target_config().default_call_conv,
    };
    let foo_id = module
        .declare_function("foo", Linkage::Export, &foo_sig)
        .unwrap();

    ctx.func.signature = foo_sig;
    ctx.func.name = UserFuncName::testcase("foo");

    // Create the builder to build a function.
    let mut builder = FunctionBuilder::new(&mut ctx.func, builder_ctx);

    // Create the entry block, to start emitting code in.
    let entry_block = builder.create_block();

    builder.switch_to_block(entry_block);
    // tell the builder that the block will have no further predecessors
    builder.seal_block(entry_block);

    let struct_arg = builder.append_block_param(entry_block, ptr_ty);

    let member_val = builder
        .ins()
        .load(types::I32, MemFlags::trusted(), struct_arg, 0);

    builder.ins().return_(&[member_val]);

    builder.seal_all_blocks();
    builder.finalize();

    println!("\n{}", ctx.func);

    module
        .define_function(foo_id, ctx)
        .expect("error defining function");

    module.clear_context(ctx);

    foo_id
}
